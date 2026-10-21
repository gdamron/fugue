//! Requests timed by a condition submitted by module id: a watched module
//! computing after the target is ordered before it, so the request applies
//! on its exact sample and nothing else changes; one the target runs into
//! is left where it is; a full table refuses at submit.

use super::request_beats::{outcomes_of, render_counted};
use super::request_conditions::{first_change, nth_beat, rig};
use super::request_order::retargeted_onto_a_later_module;
use super::*;
use crate::control_request::{Edge, Outcome, GATE_LEVEL};
use crate::invention::publish::watch::Part;
use crate::ControlValue;

/// The `count`th beat of the clock.
fn beats(count: u32) -> Part<'static> {
    Part::Signal {
        module: "clock",
        port: "beat",
        edge: Edge::Rising,
        level: GATE_LEVEL,
        count,
    }
}

/// Sets the dial's level to 1 once `part` has happened.
fn level_on(rig: &Rig, part: Part<'_>) -> Result<RequestId, GraphCommandError> {
    let one = ControlValue::Number(1.0);
    rig.live.submit_when(("dial", "level"), &one, &[part], None)
}

fn watches(rig: &Rig) -> Vec<(String, String)> {
    let publisher = rig.live.publisher().lock().unwrap();
    publisher.mirror().watches.clone()
}

#[test]
fn a_watched_module_computing_later_is_ordered_first() {
    // Added first, the clock computes after the dial.
    let oracle = rig(false).render(200);
    let mut rig = rig(false);
    let generation = rig.live.generation();
    let id = level_on(&rig, beats(2)).unwrap();
    assert_eq!(rig.live.generation(), generation + 1);
    assert_eq!(watches(&rig), [("clock".into(), "dial".into())]);
    assert_eq!(rig.live.pending_writes()[0].key, "level");

    let out = render_counted(&mut rig, 200);
    let beat = nth_beat(&oracle, 2);
    assert_ne!(beat % 64, 0, "the beat falls inside a block");
    // The order changes nothing else: the render matches up to the beat.
    assert_eq!(first_change(&out, &oracle), beat);
    assert_eq!(
        outcomes_of(&mut rig, id),
        [(id, Outcome::Applied { at: beat })]
    );
    let log = rig.live.request_log();
    assert_eq!(log.requests.len(), 1);
}

#[test]
fn a_control_or_a_request_watched_orders_nothing() {
    // Read at segment starts, or settled before the target computes on.
    let rig = rig(false);
    let generation = rig.live.generation();
    let tempo = Part::Control {
        module: "clock",
        key: "bpm",
        edge: Edge::Rising,
        level: 200.0,
    };
    let first = level_on(&rig, tempo).unwrap();
    level_on(&rig, Part::Applied(first)).unwrap();
    assert_eq!(rig.live.generation(), generation);
    assert!(watches(&rig).is_empty());
}

#[test]
fn a_watched_module_the_target_runs_into_is_left_and_applies_late() {
    let mut rig = rig(true);
    let osc = rig.build("osc", "oscillator", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("osc", osc);
            change.connect(edge("dial", "out", "osc", "frequency_mod"))
        })
        .unwrap();
    let generation = rig.live.generation();
    let rises = Part::Signal {
        module: "osc",
        port: "audio",
        edge: Edge::Rising,
        level: 0.0,
        count: 1,
    };
    let id = level_on(&rig, rises).unwrap();
    assert_eq!(rig.live.generation(), generation);
    render_counted(&mut rig, 4);
    assert!(matches!(
        outcomes_of(&mut rig, id)[..],
        [(_, Outcome::AppliedLate { .. })]
    ));
}

#[test]
fn a_request_after_a_retarget_publishes_its_order() {
    // The retarget published its recompile (FUG-388), so the request's
    // order goes in with nothing else moved.
    let oracle = retargeted_onto_a_later_module().render(200);
    let mut rig = retargeted_onto_a_later_module();
    let generation = rig.live.generation();
    level_on(&rig, beats(1_000)).unwrap();
    assert_eq!(rig.live.generation(), generation + 1);
    assert_eq!(watches(&rig), [("clock".to_string(), "dial".to_string())]);
    assert!(rig.render(200) == oracle, "unchanged while it waits");
}

#[test]
fn a_full_table_refuses_at_submit() {
    let rig = rig(true);
    for _ in 0..crate::control_request::CONDITION_CAPACITY {
        level_on(&rig, beats(1_000)).unwrap();
    }
    assert!(matches!(
        level_on(&rig, beats(1_000)),
        Err(GraphCommandError::ControlError(_))
    ));
}

#[test]
fn unknown_ports_and_controls_refuse_with_nothing_submitted() {
    let rig = rig(true);
    let mut part = beats(1);
    if let Part::Signal { port, .. } = &mut part {
        *port = "nope";
    }
    assert!(matches!(
        level_on(&rig, part),
        Err(GraphCommandError::InvalidPort(_))
    ));
    let one = ControlValue::Number(1.0);
    let missing = rig
        .live
        .submit_when(("dial", "nope"), &one, &[beats(1)], None);
    assert!(matches!(missing, Err(GraphCommandError::ControlError(_))));
    assert!(matches!(
        level_on(&rig, beats(0)),
        Err(GraphCommandError::ControlError(_))
    ));
    assert_eq!(rig.live.requests.watch_slots().taken(), 0);
}
