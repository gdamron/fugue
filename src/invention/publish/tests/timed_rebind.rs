//! A timed edit that rebuilds a module a surviving scheduler targets hands
//! the scheduler its schedule re-resolved against the edit as it lands, so
//! the schedule plays on to the new instance from the landing sample. The
//! handover is recorded only once the edit is adopted: one refused, expired
//! or cancelled records nothing and leaves the scheduler bound as before. A
//! schedule write racing the edit while it waits cancels it, as a change
//! publishing does.

use super::migrated::{read, write};
use super::request_beats::{outcomes_of, render_counted};
use super::schedule_payload::scheduler;
use super::scheduler_beats::{beat_samples, blocks, level, rig_on};
use super::*;
use crate::control_request::{BeatSpec, Outcome, Refusal, RequestId};
use crate::invention::declared::STALE;
use crate::invention::publish::timed::EditTime;
use crate::test_support::dial::DIAL;
use crate::ControlValue;

/// A tempo with fractional samples per beat at 48 kHz.
const BPM: f64 = 97.0;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
    "connections": []
}"#;

/// The level the schedule sets at `step`.
fn value(step: usize) -> f32 {
    0.1 * step as f32
}

/// A clock at `BPM` stepping a scheduler that sets the dial's level at
/// each of steps 1 to 6; the dial (level 0.25) reaches the unclipped dac.
fn stepped() -> Rig {
    let schedule: Vec<_> = (1..=6)
        .map(|step| level(step as u32, value(step)))
        .collect();
    rig_on(DOCUMENT, BPM, serde_json::json!(schedule), ("dial", "out"))
}

/// Queues a swap of the dial for a new instance (level 0.25) at `time`,
/// refused once `ttl` samples have passed.
fn swap_dial(
    rig: &Rig,
    time: EditTime<'_>,
    ttl: Option<u64>,
) -> Result<RequestId, GraphCommandError> {
    let mut change = rig.live.begin();
    change.upsert("dial", rig.build("dial", DIAL, serde_json::json!({})));
    let prepared = change.prepare().unwrap();
    rig.live.commit_when(prepared, time, ttl, |_| {})
}

/// Whether `out` holds each step's level through its beat, from `from`
/// to step 6.
fn plays_steps(out: &[f32], beats: &[usize], from: usize) -> bool {
    (from..=6).all(|step| {
        out[beats[step]..beats[step + 1]]
            .iter()
            .all(|&level| level == value(step))
    })
}

/// On a step's beat: from the landing sample the new instance plays, that
/// step's entry included (FUG-413), and every later step's.
#[test]
fn a_timed_swap_at_a_beat_plays_the_schedule_on_to_the_new_instance() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let controls = scheduler(&rig);
    let recorded = controls.recorded();
    // From the clock's beat 0, as the drain takes it in the first block.
    let time = EditTime::Beat {
        clock: "clock",
        spec: BeatSpec::After(3.0),
    };
    let id = swap_dial(&rig, time, None).unwrap();
    // Waiting: nothing recorded yet.
    assert_eq!(controls.recorded(), recorded);
    let out = render_counted(&mut rig, blocks(BPM, 7.0));

    let landed = Outcome::Applied {
        at: beats[3] as u64,
    };
    assert_eq!(outcomes_of(&mut rig, id), [(id, landed)]);
    assert!(out[..beats[1]].iter().all(|&level| level == 0.25));
    assert!(plays_steps(&out, &beats, 1), "the schedule plays on");
    // Adopted: the handover is recorded as the schedule the scheduler
    // adopted.
    rig.live.adopt_landed();
    assert_ne!(controls.recorded(), recorded);
    assert_eq!(controls.recorded(), controls.adopted());
}

/// Timed in samples onto a step's, the same: the step's entry plays once,
/// on the new instance (FUG-413).
#[test]
fn a_timed_swap_on_a_step_plays_its_entry_on_the_new_instance() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let id = swap_dial(&rig, EditTime::AtSample(beats[3] as u64), None).unwrap();
    let out = render_counted(&mut rig, blocks(BPM, 7.0));
    let landed = Outcome::Applied {
        at: beats[3] as u64,
    };
    assert_eq!(outcomes_of(&mut rig, id), [(id, landed)]);
    assert!(plays_steps(&out, &beats, 1));
}

/// An immediate swap installing on a step's sample (a block start) plays
/// that step's entry on the new instance too.
#[test]
fn an_immediate_swap_on_a_step_plays_its_entry_on_the_new_instance() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let lands = beats[3];
    let mut out = render_counted(&mut rig, lands / 64);
    let rest = lands - out.len();
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    rig.graph
        .process_block(&mut left[..rest], &mut right[..rest]);
    out.extend_from_slice(&left[..rest]);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            Ok(())
        })
        .unwrap();
    out.extend(render_counted(&mut rig, blocks(BPM, 7.0) - out.len() / 64));
    assert!(plays_steps(&out, &beats, 1));
}

/// Landing mid-step, the new instance holds its own level until the next
/// step, as an immediate swap's does, and plays every step after it.
#[test]
fn a_timed_swap_mid_step_plays_the_next_step_on_the_new_instance() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let lands = beats[2] + 100;
    let id = swap_dial(&rig, EditTime::AtSample(lands as u64), None).unwrap();
    let out = render_counted(&mut rig, blocks(BPM, 7.0));
    let landed = Outcome::Applied { at: lands as u64 };
    assert_eq!(outcomes_of(&mut rig, id), [(id, landed)]);
    assert!(out[beats[2]..lands].iter().all(|&level| level == value(2)));
    assert!(out[lands..beats[3]].iter().all(|&level| level == 0.25));
    assert!(plays_steps(&out, &beats, 3));
}

/// Expired before it is due, the edit is refused: nothing recorded, and
/// the schedule plays on the dial it was bound to.
#[test]
fn an_expired_timed_swap_records_and_rebinds_nothing() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let controls = scheduler(&rig);
    let (recorded, generation) = (controls.recorded(), rig.live.generation());
    let id = swap_dial(&rig, EditTime::AtSample(beats[3] as u64), Some(64)).unwrap();
    let out = render_counted(&mut rig, blocks(BPM, 7.0));
    let refused = Outcome::Refused(Refusal::Expired);
    assert_eq!(outcomes_of(&mut rig, id), [(id, refused)]);
    assert_eq!(rig.live.generation(), generation, "nothing adopted");
    assert_eq!(controls.recorded(), recorded);
    assert!(plays_steps(&out, &beats, 1));
}

/// A stale edit is refused with nothing queued; one cancelled by a change
/// published while it waits records nothing either.
#[test]
fn a_stale_or_cancelled_timed_swap_records_and_rebinds_nothing() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let controls = scheduler(&rig);
    let recorded = controls.recorded();

    let mut change = rig.live.begin();
    change.upsert("dial", rig.build("dial", DIAL, serde_json::json!({})));
    let stale = change.prepare().unwrap();
    let lfo = rig.build("lfo", "lfo", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("lfo", lfo);
            Ok(())
        })
        .unwrap();
    let time = EditTime::AtSample(beats[3] as u64);
    let error = rig.live.commit_when(stale, time, None, |_| {}).unwrap_err();
    assert!(matches!(error, GraphCommandError::TopologyMoved), "{error}");
    assert_eq!(controls.recorded(), recorded);

    let id = swap_dial(&rig, time, None).unwrap();
    let spare = rig.build("spare", "lfo", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("spare", spare);
            Ok(())
        })
        .unwrap();
    let out = render_counted(&mut rig, blocks(BPM, 7.0));
    let refused = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(outcomes_of(&mut rig, id), [(id, refused)]);
    assert_eq!(controls.recorded(), recorded);
    assert!(plays_steps(&out, &beats, 1));
}

/// A schedule written to the scheduler while the edit waits replaces the
/// schedule the edit re-resolved, so it cancels the edit, as a change
/// publishing does, and plays on the graph that is playing.
#[test]
fn a_schedule_written_while_a_timed_swap_waits_cancels_it() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let controls = scheduler(&rig);
    let id = swap_dial(&rig, EditTime::AtSample(beats[3] as u64), None).unwrap();
    let rewritten: Vec<_> = (1..=6)
        .map(|step| level(step as u32, value(7 - step)))
        .collect();
    controls
        .set_schedule_json(&serde_json::json!(rewritten).to_string())
        .unwrap();
    assert!(!rig.live.publisher().lock().unwrap().timed_waiting());
    let out = render_counted(&mut rig, blocks(BPM, 7.0));
    let refused = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(outcomes_of(&mut rig, id), [(id, refused)]);
    let heard = (1..=6).all(|step| {
        out[beats[step]..beats[step + 1]]
            .iter()
            .all(|&level| level == value(7 - step))
    });
    assert!(heard, "the write plays on the dial it resolved");
    assert_eq!(controls.recorded(), controls.adopted());
}

/// Once the edit has landed, a schedule resolved against the graph it
/// replaced is stale until the edit is adopted, then resolves against it.
#[test]
fn a_schedule_written_after_a_timed_swap_lands_waits_for_its_adoption() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let controls = scheduler(&rig);
    let lands = beats[2] + 100;
    swap_dial(&rig, EditTime::AtSample(lands as u64), None).unwrap();
    render_counted(&mut rig, lands / 64 + 1);
    // Landed, not adopted: the write takes the publisher without adopting.
    let json = serde_json::json!([level(3, 0.9)]).to_string();
    assert_eq!(controls.set_schedule_json(&json), Err(STALE.to_string()));
    rig.live.adopt_landed();
    controls.set_schedule_json(&json).unwrap();
    let out = render_counted(&mut rig, beats[4] / 64 + 1 - (lands / 64 + 1));
    let start = (lands / 64 + 1) * 64;
    let step3 = &out[beats[3] - start..beats[4] - start];
    assert!(
        step3.iter().all(|&level| level == 0.9),
        "on the new instance"
    );
}

/// An edit swapping one of a scheduler's targets and unplugging its clock
/// on a step's sample leaves the scheduler off the drain: the step plays
/// before it installs, as one only unplugging the clock does, so its
/// entry on the target kept lands.
#[test]
fn a_swap_unplugging_the_clock_on_a_step_plays_it_before_the_install() {
    let beats = beat_samples(BPM, 8);
    let mut rig = stepped();
    let d2 = rig.build("d2", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("d2", d2);
            Ok(())
        })
        .unwrap();
    let schedule = serde_json::json!([level(3, 0.3), at_d2(3, 0.6)]);
    write(&rig, "sched", "schedule", schedule.to_string().into());
    let lands = beats[3];
    let out = render_counted(&mut rig, lands / 64);
    let rest = lands - out.len();
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    rig.graph
        .process_block(&mut left[..rest], &mut right[..rest]);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.disconnect(edge("clock", "beat", "sched", "clock"));
            Ok(())
        })
        .unwrap();
    render_counted(&mut rig, 1);
    assert_eq!(read(&rig, "d2", "level"), ControlValue::Number(0.6));
}

/// An entry at `step` setting d2's level to `value`.
fn at_d2(step: u32, value: f32) -> serde_json::Value {
    serde_json::json!({ "at_step": step, "module": "d2", "control": "level", "value": value })
}

/// Two schedulers writing d2 on one step, only the first also writing the
/// dial a swap rebuilds there: the later one in module order still lands
/// its write last, the install timed or not.
#[test]
fn tables_on_one_control_keep_their_order_across_a_swap_on_a_step() {
    let beats = beat_samples(BPM, 8);
    for timed in [false, true] {
        let mut rig = stepped();
        let d2 = rig.build("d2", DIAL, serde_json::json!({}));
        let config = serde_json::json!({ "schedule": [] });
        let sched_b = rig.build("sched_b", "control_scheduler", config);
        rig.live
            .edit(|change| {
                change.upsert("d2", d2);
                change.upsert("sched_b", sched_b);
                change.connect(edge("clock", "beat", "sched_b", "clock"))
            })
            .unwrap();
        let first = serde_json::json!([level(3, 0.3), at_d2(3, 0.5)]);
        write(&rig, "sched", "schedule", first.to_string().into());
        let last = serde_json::json!([at_d2(3, 0.75)]);
        write(&rig, "sched_b", "schedule", last.to_string().into());
        let lands = beats[3];
        if timed {
            swap_dial(&rig, EditTime::AtSample(lands as u64), None).unwrap();
            render_counted(&mut rig, lands / 64 + 1);
            // Read through the surfaces the landed edit installs.
            rig.live.adopt_landed();
        } else {
            render_counted(&mut rig, lands / 64);
            let rest = lands - rig.graph.current_sample as usize;
            let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
            rig.graph
                .process_block(&mut left[..rest], &mut right[..rest]);
            let dial = rig.build("dial", DIAL, serde_json::json!({}));
            rig.live
                .edit(|change| {
                    change.upsert("dial", dial);
                    Ok(())
                })
                .unwrap();
            render_counted(&mut rig, 1);
        }
        let heard = (read(&rig, "d2", "level"), read(&rig, "dial", "level"));
        let expected = (ControlValue::Number(0.75), ControlValue::Number(0.3));
        assert_eq!(heard, expected, "timed: {timed}");
    }
}

/// A ramp of the dial's level from step 1 arriving at 1.0 on step 3, the
/// sample a swap of the dial installs at, immediately or timed: its last
/// write reaches the new instance, which holds 1.0 from there (FUG-413).
#[test]
fn a_swap_on_a_ramp_s_arrival_lands_its_end_on_the_new_instance() {
    let beats = beat_samples(BPM, 8);
    let ramp = serde_json::json!([
        { "at_step": 1, "module": "dial", "control": "level", "value": 1.0, "ramp_steps": 2 }
    ]);
    for timed in [false, true] {
        let mut rig = rig_on(DOCUMENT, BPM, ramp.clone(), ("dial", "out"));
        let lands = beats[3];
        let mut out = if timed {
            swap_dial(&rig, EditTime::AtSample(lands as u64), None).unwrap();
            Vec::new()
        } else {
            let mut out = render_counted(&mut rig, lands / 64);
            let rest = lands - out.len();
            let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
            rig.graph
                .process_block(&mut left[..rest], &mut right[..rest]);
            out.extend_from_slice(&left[..rest]);
            let dial = rig.build("dial", DIAL, serde_json::json!({}));
            rig.live
                .edit(|change| {
                    change.upsert("dial", dial);
                    Ok(())
                })
                .unwrap();
            out
        };
        out.extend(render_counted(&mut rig, blocks(BPM, 5.0) - out.len() / 64));
        let held = &out[lands..beats[5]];
        assert!(held.iter().all(|&level| level == 1.0), "timed: {timed}");
    }
}
