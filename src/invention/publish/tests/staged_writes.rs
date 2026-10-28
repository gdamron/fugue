//! A request's plain write is staged as it falls due and its setter runs as
//! its module processes (see `control_request::staged`): heard on its
//! sample, sample-accurate within a block, settled as it applies, and
//! flushed before whatever needs it applied first: a zero-length block's
//! end, a feedback loop's samples, an edit landing on its sample, a condition chained on it, a clock's
//! beats. Each test here fails with its flush taken out.

use super::request_beats::{beat_rig, outcomes_of, render_counted, submit_level, BPM};
use super::request_conditions::rig as condition_rig;
use super::requests::{outcomes, submit, try_submit_value};
use super::*;
use crate::control_request::{
    BeatSpec, Condition, ControlIndex, Outcome, Predicate, Rank, Refusal, Request, RequestValue,
    RtValue, Source, When,
};
use crate::invention::publish::timed::EditTime;
use crate::test_support::dial::{DialFactory, DIAL, HELD, LEVEL, SHAPE};

/// An oscillator's `frequency`.
const FREQUENCY: u16 = 0;

/// A dial (level 0.25) alone into an unclipped dac.
fn dial_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
            "version": "1.0.0",
            "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
            "connections": []
        }"#,
    );
    rig.registry.register(DialFactory);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig.render(1);
    rig
}

/// A block of no frames: requests settle, nothing processes.
fn empty_block(rig: &mut Rig) {
    rig.graph.process_block(&mut [], &mut []);
}

fn level(rig: &Rig) -> Option<RtValue> {
    let (_, cells) = rig.graph.modules["dial"].module().declared().unwrap();
    cells.load(LEVEL)
}

#[test]
fn writes_at_different_samples_of_one_block_each_play_from_their_own() {
    let mut rig = dial_rig();
    let start = rig.graph.current_sample;
    let now = submit(&rig, "dial", LEVEL.0, 0.5, When::Now);
    let at_10 = submit(&rig, "dial", LEVEL.0, 0.75, When::AtSample(start + 10));
    let at_40 = submit(&rig, "dial", LEVEL.0, 1.0, When::AtSample(start + 40));
    let out = render_counted(&mut rig, 1);
    assert!(out[..10].iter().all(|&v| v == 0.5), "{out:?}");
    assert!(out[10..40].iter().all(|&v| v == 0.75), "{out:?}");
    assert!(out[40..].iter().all(|&v| v == 1.0), "{out:?}");
    let mut settled = outcomes(&mut rig);
    settled.sort_by_key(|(id, _)| *id);
    assert_eq!(
        settled,
        [
            (now, Outcome::Applied { at: start }),
            (at_10, Outcome::Applied { at: start + 10 }),
            (at_40, Outcome::Applied { at: start + 40 }),
        ]
    );
}

#[test]
fn a_zero_length_block_settles_what_it_staged() {
    let mut rig = dial_rig();
    let start = rig.graph.current_sample;
    let id = submit(&rig, "dial", LEVEL.0, 0.75, When::Now);
    empty_block(&mut rig);
    assert_eq!(outcomes(&mut rig), [(id, Outcome::Applied { at: start })]);
    assert_eq!(level(&rig), Some(RtValue::F32(0.75)), "read back");
}

#[test]
fn a_write_refused_as_it_flushes_settles_refused_and_gives_back_its_hold() {
    let mut rig = dial_rig();
    let shape = RequestValue::Value(RtValue::U32(5));
    let id = try_submit_value(&rig, "dial", SHAPE.0, shape, When::Now).unwrap();
    empty_block(&mut rig);
    let refused = Outcome::Refused(Refusal::Invalid);
    assert_eq!(outcomes(&mut rig), [(id, refused)]);
    // Refused, it holds nothing: a script may still write at this sample.
    let script = Rank::new(Source::Script, 0);
    assert!(rig.graph.admits("dial", SHAPE, script));
}

/// A loop's modules compute sample by sample, unsplit: a write due inside
/// a block plays from its sample there, as one written as a block starts
/// at that sample does, and is heard in that block.
#[test]
fn a_write_inside_a_block_on_a_feedback_loop_plays_from_its_sample() {
    const AT: usize = 40;
    let mut timed = Rig::new(super::carry::LOOP);
    let mut twin = Rig::new(super::carry::LOOP);
    let start = timed.graph.current_sample;
    submit(
        &timed,
        "osc1",
        FREQUENCY,
        330.0,
        When::AtSample(start + AT as u64),
    );
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    timed.graph.process_block(&mut left, &mut right);
    let (mut twin_left, mut twin_right) = ([0.0f32; 64], [0.0f32; 64]);
    let (first, rest) = twin_left.split_at_mut(AT);
    let (first_right, rest_right) = twin_right.split_at_mut(AT);
    twin.graph.process_block(first, first_right);
    submit(&twin, "osc1", FREQUENCY, 330.0, When::Now);
    twin.graph.process_block(rest, rest_right);
    super::carry::assert_identical(&left, &twin_left, "a write at sample 40");
    // Heard from there, not from the next block.
    let mut plain = Rig::new(super::carry::LOOP);
    let unwritten = render_counted(&mut plain, 1);
    super::carry::assert_identical(&left[..AT], &unwritten[..AT], "before it");
    assert_ne!(
        left[AT..],
        unwritten[AT..],
        "the write is heard in its block"
    );
}

/// An edit landing on a write's sample installs after it applies: the
/// write applies to the module the edit removes, on its sample.
#[test]
fn a_write_on_the_sample_an_edit_lands_applies_to_the_module_it_removes() {
    let mut rig = beat_rig(120.0);
    rig.render(3);
    let at = rig.graph.current_sample + 64 * 5 + 17;
    let write = submit(&rig, "dial", LEVEL.0, 0.75, When::AtSample(at));
    let mut change = rig.live.begin();
    change.remove("dial");
    let prepared = change.prepare().unwrap();
    let edit = rig
        .live
        .commit_when(prepared, EditTime::AtSample(at), None, |state| {
            state.document = None;
        });
    let edit = edit.unwrap();
    render_counted(&mut rig, 8);
    let all = outcomes(&mut rig);
    assert!(all.contains(&(edit, Outcome::Applied { at })), "{all:?}");
    assert!(all.contains(&(write, Outcome::Applied { at })), "{all:?}");
}

/// A write chained on another applies on the sample that one applies at,
/// in the same span: the one it waits on settles as it applies, not when
/// its module next flushes.
#[test]
fn a_write_chained_on_a_condition_s_write_applies_on_its_sample() {
    let mut rig = condition_rig(true);
    let (first, then) = {
        let publisher = rig.live.publisher().lock().unwrap();
        let level = publisher.control_target("dial", LEVEL).unwrap();
        let clock = publisher.control_target("clock", ControlIndex(0)).unwrap();
        let mut first = Request::new(level, RequestValue::Value(RtValue::F32(1.0)));
        let beat = Predicate::edges(clock.module_idx as u32, 0, 2);
        first.when = When::Condition(Condition::all(&[beat]).unwrap());
        let first = rig.live.requests.submit(first).unwrap();
        let held = publisher.control_target("dial", HELD).unwrap();
        let mut then = Request::new(held, RequestValue::Value(RtValue::Bool(true)));
        then.when = When::Condition(Condition::all(&[Predicate::Applied(first)]).unwrap());
        (first, rig.live.requests.submit(then).unwrap())
    };
    render_counted(&mut rig, 200);
    let all = outcomes(&mut rig);
    let of = |id| all.iter().find(|(got, _)| *got == id).unwrap().1;
    let Outcome::Applied { at } = of(first) else {
        panic!("{all:?}");
    };
    assert_ne!(at % 64, 0, "the beat falls inside a block");
    assert_eq!(of(then), Outcome::Applied { at });
}

/// A tempo written for a sample settles before the beats are placed at
/// it: a beat counted from there falls at the new tempo, on time.
#[test]
fn a_tempo_written_for_a_sample_places_the_beats_at_it() {
    // At 60 bpm the clock's first beat falls at 47 999; from 47 000, at
    // 6 000 bpm, it falls ten samples later.
    let mut rig = beat_rig(60.0);
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("clock", BPM).unwrap();
    let mut faster = Request::new(target, RequestValue::Value(RtValue::F32(6_000.0)));
    faster.when = When::AtSample(47_000);
    rig.live.requests.submit(faster).unwrap();
    drop(publisher);
    let beat = submit_level(&rig, 0.5, BeatSpec::After(1.0));
    render_counted(&mut rig, 47_100 / 64);
    let outcome = outcomes_of(&mut rig, beat);
    assert!(
        matches!(outcome[..], [(_, Outcome::Applied { at })] if (47_001..47_040).contains(&at)),
        "{outcome:?}"
    );
}
