//! Requests timed by a condition on a live graph: the nth beat of a clock
//! sets a dial's level on that beat's exact sample when the clock computes
//! before the dial, and before the dial's next segment when it computes
//! after; a removed source refuses the request; every block stays
//! allocation- and free-free. The same graph rendered without the request
//! is the oracle.

use super::request_beats::{outcomes_of, render_counted};
use super::*;
use crate::control_request::{
    Condition, ControlIndex, Edge, Outcome, Predicate, Refusal, Request, RequestId, RequestValue,
    RtValue, When,
};
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};

/// Beats fall inside 64-frame blocks at this tempo, never on their edge.
const BPM: f64 = 1100.0;

/// A clock and a dial (level 0.25) into an unclipped dac, added in the
/// given order: the module added later computes first.
fn rig(clock_first: bool) -> Rig {
    let mut rig = Rig::new(
        r#"{
        "version": "1.0.0",
        "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
        "connections": []
    }"#,
    );
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": BPM }));
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            if clock_first {
                change.upsert("dial", dial);
                change.upsert("clock", clock);
            } else {
                change.upsert("clock", clock);
                change.upsert("dial", dial);
            }
            change.connect(edge("clock", "beat", "dac", "audio"))?;
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig
}

/// Sets the dial's level to 1 on the clock's `beats`th beat from now.
fn submit_on_beat(rig: &Rig, beats: u32) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", LEVEL).unwrap();
    let clock = publisher.control_target("clock", ControlIndex(0)).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(1.0)));
    let beat = Predicate::edges(clock.module_idx as u32, 0, beats);
    request.when = When::Condition(Condition::all(&[beat]).unwrap());
    rig.live.requests.submit(request).unwrap()
}

/// The sample of the clock's `n`th beat (from one) in `out`, a render of
/// the rig without requests: its gate rising over the dial's 0.25.
fn nth_beat(out: &[f32], n: usize) -> u64 {
    let mut last = 0.0;
    let mut rises = out.iter().enumerate().filter(|&(_, &sample)| {
        let gate = sample - 0.25;
        let rose = last <= 0.5 && gate > 0.5;
        last = gate;
        rose
    });
    rises.nth(n - 1).unwrap().0 as u64
}

/// The first sample `out` differs from `oracle` at.
fn first_change(out: &[f32], oracle: &[f32]) -> u64 {
    let mut pairs = out.iter().zip(oracle);
    pairs.position(|(a, b)| a != b).unwrap() as u64
}

#[test]
fn a_condition_on_a_module_computed_first_applies_on_its_exact_sample() {
    let oracle = rig(true).render(200);
    let mut rig = rig(true);
    rig.graph.ensure_process_order();
    let clock = rig.graph.modules.get_index_of("clock").unwrap();
    let dial = rig.graph.modules.get_index_of("dial").unwrap();
    assert!(rig.graph.group_of[clock] < rig.graph.group_of[dial]);

    let id = submit_on_beat(&rig, 3);
    let out = render_counted(&mut rig, 200);
    let beat = nth_beat(&oracle, 3);
    assert_ne!(beat % 64, 0, "the beat falls inside a block");
    assert_eq!(first_change(&out, &oracle), beat);
    assert_eq!(out[beat as usize] - oracle[beat as usize], 0.75);
    assert_eq!(
        outcomes_of(&mut rig, id),
        [(id, Outcome::Applied { at: beat })]
    );
}

#[test]
fn a_condition_on_a_module_computed_later_applies_at_the_next_segment() {
    let oracle = rig(false).render(200);
    let mut rig = rig(false);
    let id = submit_on_beat(&rig, 2);
    let out = render_counted(&mut rig, 200);
    let beat = nth_beat(&oracle, 2);
    let next_block = (beat / 64 + 1) * 64;
    assert_eq!(first_change(&out, &oracle), next_block);
    assert_eq!(
        outcomes_of(&mut rig, id),
        [(
            id,
            Outcome::AppliedLate {
                at: next_block,
                due: beat
            }
        )]
    );
}

#[test]
fn removing_a_source_refuses_the_request() {
    let mut rig = rig(true);
    let id = submit_on_beat(&rig, 1_000);
    render_counted(&mut rig, 4);
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();
    render_counted(&mut rig, 2);
    assert_eq!(
        outcomes_of(&mut rig, id),
        [(id, Outcome::Refused(Refusal::SourceGone))]
    );
    assert_eq!(rig.live.requests.watch_slots().taken(), 0);
}

#[test]
fn a_settled_condition_frees_its_slot() {
    let mut rig = rig(true);
    let id = submit_on_beat(&rig, 1);
    assert_eq!(rig.live.requests.watch_slots().taken(), 1);
    render_counted(&mut rig, 200);
    assert!(matches!(
        outcomes_of(&mut rig, id)[..],
        [(_, Outcome::Applied { .. })]
    ));
    assert_eq!(rig.live.requests.watch_slots().taken(), 0);
}

/// Submits `value` for `control` of `module_id`, timed `when`.
fn submit_when(rig: &Rig, (module_id, control): (&str, ControlIndex), when: When) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target(module_id, control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(1.0)));
    request.when = when;
    rig.live.requests.submit(request).unwrap()
}

/// The module index of `module_id` in the publisher's order.
fn index_of(rig: &Rig, module_id: &str) -> u32 {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher
        .control_target(module_id, ControlIndex(0))
        .unwrap();
    target.module_idx as u32
}

#[test]
fn a_control_crossing_applies_at_the_next_segment_start() {
    let mut rig = rig(true);
    let dial = index_of(&rig, "dial");
    let crossing = Predicate::Control {
        module: dial,
        control: LEVEL,
        edge: Edge::Rising,
        level: 0.5,
    };
    let when = When::Condition(Condition::all(&[crossing]).unwrap());
    let waits = submit_when(&rig, ("clock", ControlIndex(0)), when);
    submit_when(&rig, ("dial", LEVEL), When::AtSample(100));
    render_counted(&mut rig, 4);
    // The level changes at 100, inside the second block: read at the next
    // segment start, the third block's.
    assert_eq!(
        outcomes_of(&mut rig, waits),
        [(waits, Outcome::Applied { at: 128 })]
    );
}

#[test]
fn a_request_waiting_on_another_applies_on_its_sample() {
    let mut rig = rig(true);
    let first = submit_when(&rig, ("dial", LEVEL), When::AtSample(100));
    let after = Predicate::Applied(first);
    let when = When::Condition(Condition::all(&[after]).unwrap());
    let then = submit_when(&rig, ("clock", ControlIndex(0)), when);
    render_counted(&mut rig, 4);
    assert_eq!(
        outcomes_of(&mut rig, then),
        [(then, Outcome::Applied { at: 100 })]
    );
}

#[test]
fn a_tempo_a_chain_sets_places_the_next_beat_again() {
    use super::request_beats::{beat_rig, submit_level, BPM};
    use crate::control_request::BeatSpec;
    // At 60 bpm the clock's first beat falls at 47 999; from 47 000, at
    // 6 000 bpm, it falls ten samples later.
    let mut rig = beat_rig(60.0);
    let first = submit_when(&rig, ("dial", LEVEL), When::AtSample(47_000));
    let after = Condition::all(&[Predicate::Applied(first)]).unwrap();
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("clock", BPM).unwrap();
    let mut faster = Request::new(target, RequestValue::Value(RtValue::F32(6_000.0)));
    faster.when = When::Condition(after);
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

#[test]
fn a_chain_stopping_a_clock_on_its_beat_stops_it_before_the_beat() {
    use super::request_beats::{beat_rig, submit_level, BPM};
    use crate::control_request::BeatSpec;
    // At 60 bpm the clock's first beat falls at 47 999, where a stop timed
    // in samples keeps it from beating: so must one chained to apply there.
    let mut rig = beat_rig(60.0);
    let first = submit_when(&rig, ("dial", LEVEL), When::AtSample(47_999));
    let after = Condition::all(&[Predicate::Applied(first)]).unwrap();
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("clock", BPM).unwrap();
    let mut stop = Request::new(target, RequestValue::Value(RtValue::F32(0.0)));
    stop.when = When::Condition(after);
    let stop = rig.live.requests.submit(stop).unwrap();
    drop(publisher);
    let beat = submit_level(&rig, 0.5, BeatSpec::After(1.0));
    render_counted(&mut rig, 48_200 / 64);
    let all = super::requests::outcomes(&mut rig);
    let of = |id| all.iter().filter(move |(got, _)| *got == id);
    assert!(of(stop).eq([&(stop, Outcome::Applied { at: 47_999 })]));
    assert_eq!(of(beat).count(), 0, "a stopped clock beats no more");
}

#[test]
fn a_request_waited_on_is_remembered_however_long_intake_is_held_up() {
    use crate::invention::publish::publisher::PENDING_REQUEST_CAPACITY;
    use crate::test_support::dial::PULSE;
    let mut rig = rig(true);
    let pulse = |rig: &Rig| {
        let publisher = rig.live.publisher().lock().unwrap();
        let target = publisher.control_target("dial", PULSE).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::Bool(true)));
        request.when = When::AtSample(1_000);
        request.event = true;
        rig.live.requests.submit(request).unwrap()
    };
    // A full store of events due together holds up what comes behind it:
    // one more, then a condition waiting on the first of them.
    let first = pulse(&rig);
    for n in 1..PENDING_REQUEST_CAPACITY {
        if n % 128 == 0 {
            render_counted(&mut rig, 1);
        }
        pulse(&rig);
    }
    render_counted(&mut rig, 1);
    pulse(&rig);
    let after = When::Condition(Condition::all(&[Predicate::Applied(first)]).unwrap());
    let waits = submit_when(&rig, ("dial", LEVEL), after);
    render_counted(&mut rig, 20);
    assert!(matches!(
        outcomes_of(&mut rig, waits)[..],
        [(_, Outcome::AppliedLate { due: 1_000, .. })]
    ));
    assert_eq!(rig.live.requests.watch_slots().taken(), 0);
}
