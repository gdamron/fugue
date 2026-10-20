//! A top-level `control_scheduler` played by the graph's request drain on a
//! live graph: each entry lands on its step's exact sample, a ramp glides
//! to land on its end step, the `step` output follows the clock, a reset of
//! the clock plays the schedule again, an entry later than a conditional
//! write in one segment takes the control back (FUG-386), two schedulers'
//! writes at one sample land in module order, and no block allocates,
//! frees or takes a lock a control thread holds.

use super::migrated::{block_with_locks_held, read, write};
use super::request_beats::{render_counted, Twin};
use super::*;
use crate::control_request::{Condition, ControlIndex, Predicate, Request, RequestValue, RtValue};
use crate::control_request::{Source, Timeline, When};
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};
use crate::ControlValue;

/// A tempo with fractional samples per beat at 48 kHz.
const BPM: f64 = 97.0;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
    "connections": []
}"#;

/// An entry at `step` writing `value` to `module`'s `control`.
fn at(step: u32, module: &str, control: &str, value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "at_step": step, "module": module, "control": control, "value": value })
}

fn level(step: u32, value: f32) -> serde_json::Value {
    at(step, "dial", "level", value.into())
}

/// A dial (level 0.25) and a clock at `bpm` stepping a scheduler playing
/// `schedule`; the scheduler's `heard` output reaches the unclipped dac.
fn rig(bpm: f64, schedule: serde_json::Value, heard: (&str, &str)) -> Rig {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": bpm }));
    let config = serde_json::json!({ "schedule": schedule });
    let sched = rig.build("sched", "control_scheduler", config);
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("clock", clock);
            change.upsert("sched", sched);
            change.connect(edge("clock", "beat", "sched", "clock"))?;
            change.connect(edge(heard.0, heard.1, "dac", "audio"))
        })
        .unwrap();
    rig
}

/// The sample each of beats `0..beats` falls on at `bpm`.
fn beat_samples(bpm: f64, beats: u32) -> Vec<usize> {
    let mut twin = Twin::new(bpm);
    let mut samples = vec![0];
    for beat in 1..beats {
        let at = twin.until(|clock| clock.position() >= f64::from(beat));
        samples.push(at as usize);
    }
    samples
}

/// Blocks of 64 through beat `beats` at `bpm`.
fn blocks(bpm: f64, beats: f64) -> usize {
    (beats * 48_000.0 * 60.0 / bpm / 64.0) as usize + 2
}

#[test]
fn each_entry_lands_on_its_step_s_sample_and_a_ramp_on_its_end_step() {
    let schedule = serde_json::json!([
        level(1, 0.5),
        { "at_step": 2, "module": "dial", "control": "level", "value": 1.0, "ramp_steps": 2 },
        level(5, 0.3)
    ]);
    let beats = beat_samples(BPM, 7);
    let mut rig = rig(BPM, schedule, ("dial", "out"));
    let out = render_counted(&mut rig, blocks(BPM, 6.0));
    assert!(out[..beats[1]].iter().all(|&level| level == 0.25));
    assert!(out[beats[1]..beats[2]].iter().all(|&level| level == 0.5));
    let glide = &out[beats[2]..beats[4]];
    assert!(glide.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(glide[0] < 0.51 && *glide.last().unwrap() < 1.0);
    assert!(out[beats[4]..beats[5]].iter().all(|&level| level == 1.0));
    assert!(out[beats[5]..].iter().all(|&level| level == 0.3));
}

#[test]
fn the_step_output_follows_the_clock_on_each_step_s_sample() {
    let beats = beat_samples(BPM, 6);
    let mut rig = rig(BPM, serde_json::json!([level(9, 0.5)]), ("sched", "step"));
    let out = render_counted(&mut rig, blocks(BPM, 5.0));
    for (step, span) in beats.windows(2).enumerate() {
        let span = &out[span[0]..span[1]];
        assert!(span.iter().all(|&at| at == step as f32), "step {step}");
    }
}

#[test]
fn a_reset_of_the_clock_plays_the_schedule_again_from_step_0() {
    let schedule = serde_json::json!([level(0, 0.5), level(1, 0.75)]);
    let beats = beat_samples(BPM, 3);
    let mut rig = rig(BPM, schedule, ("dial", "out"));
    let before = render_counted(&mut rig, beats[2] / 64);
    assert_eq!(before.last(), Some(&0.75), "played out");
    // The reset applies at the next block: the clock's beat 0 is its
    // first sample, and the schedule plays again from step 0 there.
    write(&rig, "clock", "reset", true.into());
    let after = render_counted(&mut rig, blocks(BPM, 2.0));
    // Its beat 1, as the clock run on its own with the reset there says.
    let reset = before.len() as u64;
    let mut twin = Twin::new(BPM);
    twin.writes
        .push((reset, ControlIndex(2), RtValue::Bool(true)));
    twin.run_to(reset);
    let beat = (twin.until(|clock| clock.position() >= 1.0) - reset) as usize;
    assert!(after[..beat].iter().all(|&level| level == 0.5));
    assert!(after[beat..].iter().all(|&level| level == 0.75));
}

#[test]
fn an_entry_later_than_a_conditional_write_in_one_segment_takes_the_control_back() {
    // FUG-386. The condition, the 2nd beat of a clock at 1120 bpm, falls
    // early in a block; the scheduler's step 1, on its own clock at 1107
    // bpm, later in the same block.
    let (a, b) = (1_120.0, 1_107.0);
    let (condition, entry) = (beat_samples(a, 2)[1], beat_samples(b, 2)[1]);
    assert_eq!(condition / 64, entry / 64, "one block");
    assert!(condition < entry);
    let render = |write: bool| {
        let mut rig = rig(b, serde_json::json!([level(1, 0.5)]), ("dial", "out"));
        let cond = rig.build("cond", "clock", serde_json::json!({ "bpm": a }));
        rig.live
            .edit(|change| {
                // Added last, it computes before the dial.
                change.upsert("cond", cond);
                Ok(())
            })
            .unwrap();
        if write {
            let publisher = rig.live.publisher().lock().unwrap();
            let target = publisher.control_target("dial", LEVEL).unwrap();
            let clock = publisher.control_target("cond", ControlIndex(0)).unwrap();
            let mut request = Request::new(target, RequestValue::Value(RtValue::F32(1.0)));
            request.source = Source::User;
            let beat = Predicate::edges(clock.module_idx as u32, 0, 2);
            request.when = When::Condition(Condition::all(&[beat]).unwrap());
            rig.live.requests.submit(request).unwrap();
        }
        render_counted(&mut rig, entry / 64 + 4)
    };
    let (oracle, out) = (render(false), render(true));
    assert_eq!(out[condition - 1], 0.25);
    assert_eq!(out[condition..entry], vec![1.0; entry - condition][..]);
    assert_eq!(out[entry..], oracle[entry..], "the entry takes it back");
}

/// Schedulers `sched_a` then `sched_b` (in module order) on one clock,
/// writing the dial's level at step 1: 0.5 and 0.75.
fn rivals() -> Rig {
    let rig = rig(BPM, serde_json::json!([]), ("dial", "out"));
    let rival = |id: &str, value: f32| {
        let config = serde_json::json!({ "schedule": [level(1, value)] });
        rig.build(id, "control_scheduler", config)
    };
    let (a, b) = (rival("sched_a", 0.5), rival("sched_b", 0.75));
    rig.live
        .edit(|change| {
            change.upsert("sched_a", a);
            change.upsert("sched_b", b);
            change.connect(edge("clock", "beat", "sched_b", "clock"))?;
            change.connect(edge("clock", "beat", "sched_a", "clock"))
        })
        .unwrap();
    rig
}

#[test]
fn two_schedulers_writing_at_one_sample_land_in_module_order() {
    let beats = beat_samples(BPM, 3);
    let mut rig = rivals();
    // A schedule written to sched_a re-adopts it in place: still first.
    let schedule = serde_json::json!([level(1, 0.5)]).to_string();
    write(&rig, "sched_a", "schedule", schedule.into());
    let out = render_counted(&mut rig, blocks(BPM, 2.0));
    assert_eq!(out[beats[1] - 1], 0.25);
    assert_eq!(out[beats[1]], 0.75, "sched_b, the later module, lands");
    assert_eq!(read(&rig, "dial", "level"), ControlValue::Number(0.75));
}

#[test]
fn a_block_playing_a_schedule_takes_no_lock_a_control_thread_holds() {
    let schedule = serde_json::json!([
        { "at_step": 0, "module": "dial", "control": "level", "value": 1.0, "ramp_steps": 8 }
    ]);
    let mut rig = rig(BPM, schedule, ("dial", "out"));
    render_counted(&mut rig, 4);
    block_with_locks_held(&mut rig, &[]).unwrap();
}
