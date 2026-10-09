//! A module with declared controls on a live graph: requests reach its
//! `apply` through the drain, on the audio thread, without allocating, and
//! what each control then holds is published to its cells.

use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::{
    ControlIndex, Outcome, Refusal, Request, RequestId, RequestValue, RtValue, When,
};
use crate::test_support::dial::{DialFactory, DIAL, LEVEL, PULSE, SHAPE};

/// A dial alone into an unclipped dac, so the output is what it holds.
pub(super) fn dial_rig() -> Rig {
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

/// Submits `value` for `control` of the dial as a front door would.
fn submit(rig: &Rig, control: ControlIndex, value: RtValue, at: u64, event: bool) -> RequestId {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(value));
    request.when = When::AtSample(at);
    request.event = event;
    let id = rig.live.requests.submit(request).unwrap();
    publisher.note_written();
    id
}

fn held(rig: &Rig, control: ControlIndex) -> RtValue {
    let module = rig.graph.modules["dial"].module();
    module.declared().unwrap().1.load(control).unwrap()
}

#[test]
fn a_request_reaches_apply_at_its_sample_and_is_read_back() {
    let mut rig = dial_rig();
    let start = rig.graph.current_sample;
    let id = submit(&rig, LEVEL, RtValue::F32(2.0), start + 10, false);
    assert_eq!(held(&rig, LEVEL), RtValue::F32(0.25));

    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let ((), allocs, frees) =
        crate::alloc_counter::allocator_events(|| rig.graph.process_block(&mut left, &mut right));
    assert_eq!((allocs, frees), (0, 0));
    assert!(left[..10].iter().all(|v| *v == 0.25), "{left:?}");
    assert!(left[10..].iter().all(|v| *v == 1.0), "{left:?}");
    assert_eq!(held(&rig, LEVEL), RtValue::F32(1.0), "the clamped value");
    let applied = Outcome::Applied { at: start + 10 };
    assert_eq!(outcomes(&mut rig), [(id, applied)]);
}

#[test]
fn a_value_the_module_cannot_hold_is_refused_and_changes_nothing() {
    let mut rig = dial_rig();
    let start = rig.graph.current_sample;
    let id = submit(&rig, SHAPE, RtValue::U32(2), start, false);
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(held(&rig, SHAPE), RtValue::U32(0));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Refused(Refusal::Invalid))]
    );
}

#[test]
fn two_events_at_one_sample_both_fire_where_values_coalesce() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample + 5;
    let pulses = [true, true].map(|event| submit(&rig, PULSE, RtValue::Bool(true), at, event));
    let first = submit(&rig, LEVEL, RtValue::F32(0.5), at, false);
    let last = submit(&rig, LEVEL, RtValue::F32(0.75), at, false);
    assert_eq!(counted_block(&mut rig), (0, 0));
    let out = rig.render(1);
    assert!(out.iter().all(|v| *v == 2.75), "{out:?}");
    let applied = Outcome::Applied { at };
    assert_eq!(
        outcomes(&mut rig),
        [
            (first, Outcome::Superseded),
            (pulses[0], applied),
            (pulses[1], applied),
            (last, applied),
        ]
    );
}
