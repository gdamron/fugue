//! A timed edit batch's control writes on an offline render play as the
//! daemon's do: those to a module it keeps from the sample it lands on,
//! those to a module it adds as it is built, all recorded in the document
//! once it is adopted. Cancelled, it plays none of them. An authored write
//! to a control it writes, made while it waits, is kept over it, so the
//! author's value plays on. The render lands such a batch without
//! allocating.

use super::timed_edit_tests::{daemon, edits, next_bar, unnumbered, whens, ADD, SWAP};
use super::timed_write_tests::{daemon_render, engine, render, AFTER, BLOCK, LEAD_IN};
use crate::alloc_counter::allocator_events;
use crate::rpc::{ControlRequestState, RequestState};
use crate::{ControlValue, ControlWrite, ControlWriteIntent};

/// Adds a voice an octave down and retunes it, and retunes the sine the
/// batch keeps.
const WRITES: &str = r#"[
    {"op":"add_module","id":"low","module_type":"oscillator","config":{"frequency":110.0}},
    {"op":"connect","from":"low","from_port":"audio","to":"dac","to_port":"audio"},
    {"op":"set_control","module_id":"low","key":"frequency","value":55.0},
    {"op":"set_control","module_id":"osc","key":"frequency","value":330.0}
]"#;

/// The frequency the document records for `module_id`.
fn recorded(document: Option<crate::Invention>, module_id: &str) -> Option<f64> {
    let document = document.unwrap();
    let module = document.modules.iter().find(|m| m.id == module_id)?;
    module.config["frequency"].as_f64()
}

fn author(value: f32) -> ControlWrite {
    ControlWrite {
        intent: ControlWriteIntent::Author,
        ..ControlWrite::new("osc", "frequency", ControlValue::Number(value))
    }
}

#[test]
fn its_writes_land_with_it_on_the_daemons_sample_and_are_recorded() {
    for (when, _) in whens() {
        let (mut running, pump) = daemon();
        let mut engine = engine();
        render(&mut engine, LEAD_IN);
        daemon_render(&pump, LEAD_IN);
        let daemon_queued = running.apply_edits_at(&edits(WRITES), &when, None).unwrap();
        let queued = engine.apply_edits_at(&edits(WRITES), &when, None).unwrap();
        assert_eq!(
            unnumbered(queued.clone()),
            unnumbered(daemon_queued.clone())
        );
        // Nothing of it before it lands.
        assert_eq!(
            engine.get_control("osc", "frequency").unwrap(),
            220.0.into()
        );
        let output = render(&mut engine, AFTER);
        assert_eq!(output, daemon_render(&pump, AFTER), "{when:?}");
        let adopted = engine.request_status(&queued.id).unwrap();
        assert_eq!(adopted.state, RequestState::Adopted, "{when:?}");
        let daemon_adopted = running.request_status(&daemon_queued.id).unwrap();
        assert_eq!(unnumbered(adopted.clone()), unnumbered(daemon_adopted));
        for module_id in ["osc", "low"] {
            let document = recorded(engine.document(), module_id);
            assert_eq!(
                document,
                recorded(running.document(), module_id),
                "{module_id}"
            );
        }
        assert_eq!(recorded(engine.document(), "osc"), Some(330.0));
        assert_eq!(
            engine.get_control("osc", "frequency").unwrap(),
            330.0.into()
        );

        // Its write to the sine is heard from the sample it lands on.
        let mut twin = self::engine();
        render(&mut twin, LEAD_IN);
        twin.apply_edits_at(&edits(ADD), &when, None).unwrap();
        let unwritten = render(&mut twin, AFTER);
        let landed = adopted.landing.sample.expect("landed on a sample");
        let first = (0..output.len()).find(|&i| output[i] != unwritten[i]);
        let first = first.map(|i| (LEAD_IN * BLOCK + i / 2) as u64);
        assert!(first.is_some_and(|first| first >= landed), "{when:?}");
        assert!(first.is_some_and(|first| first <= landed + 1), "{when:?}");
    }
}

#[test]
fn cancelled_it_plays_none_of_its_writes() {
    let (mut running, pump) = daemon();
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    daemon_render(&pump, LEAD_IN);
    running
        .apply_edits_at(&edits(WRITES), &next_bar(), None)
        .unwrap();
    let queued = engine
        .apply_edits_at(&edits(WRITES), &next_bar(), None)
        .unwrap();
    running.apply_edits(&edits(SWAP)).unwrap();
    engine.apply_edits(&edits(SWAP)).unwrap();
    let cancelled = engine.request_status(&queued.id).unwrap();
    assert_eq!(cancelled.state, RequestState::Cancelled);
    // Its write to the sine is refused with it at the next block, not
    // left waiting for the downbeat it would have landed on.
    assert_eq!(render(&mut engine, 1), daemon_render(&pump, 1));
    let settled = engine.take_settled_writes();
    let write = settled.iter().find(|s| s.module_id == "osc" && s.key == "frequency");
    let write = write.expect("its write settled");
    assert_eq!(write.state, ControlRequestState::Refused, "{write:?}");
    assert_eq!(render(&mut engine, AFTER), daemon_render(&pump, AFTER));
    assert_eq!(engine.get_control("osc", "frequency").unwrap(), 220.0.into());
    assert_eq!(recorded(engine.document(), "osc"), Some(220.0));
    assert_eq!(recorded(engine.document(), "low"), None);
}

#[test]
fn an_authored_write_to_a_control_it_writes_is_kept_over_it() {
    let (mut running, pump) = daemon();
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    daemon_render(&pump, LEAD_IN);
    let daemon_queued = running
        .apply_edits_at(&edits(WRITES), &next_bar(), None)
        .unwrap();
    let queued = engine
        .apply_edits_at(&edits(WRITES), &next_bar(), None)
        .unwrap();
    // A drag: each write replaces the copy kept before it.
    for value in [400.0, 420.0, 440.0] {
        running.write_controls(&[author(value)]);
        engine.write_controls(&[author(value)]).unwrap();
        assert_eq!(render(&mut engine, 4), daemon_render(&pump, 4), "{value}");
    }
    let waiting = engine.request_status(&queued.id).unwrap();
    assert_eq!(waiting.state, RequestState::Queued, "kept, not cancelled");
    let daemon_waiting = running.request_status(&daemon_queued.id).unwrap();
    assert_eq!(unnumbered(waiting), unnumbered(daemon_waiting));
    assert_eq!(render(&mut engine, AFTER), daemon_render(&pump, AFTER));
    let adopted = engine.request_status(&queued.id).unwrap();
    assert_eq!(adopted.state, RequestState::Adopted);
    let daemon_adopted = running.request_status(&daemon_queued.id).unwrap();
    assert_eq!(unnumbered(adopted), unnumbered(daemon_adopted));
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        440.0.into()
    );
    assert_eq!(
        recorded(engine.document(), "osc"),
        recorded(running.document(), "osc")
    );
    assert_eq!(recorded(engine.document(), "osc"), Some(440.0));
}

#[test]
fn a_batch_of_writes_alone_is_refused_as_the_daemon_refuses_it() {
    let (mut running, _pump) = daemon();
    let engine = engine();
    let alone = r#"[{"op":"set_control","module_id":"osc","key":"frequency","value":330.0}]"#;
    let daemon_refused = running
        .apply_edits_at(&edits(alone), &next_bar(), None)
        .unwrap_err();
    let refused = engine
        .apply_edits_at(&edits(alone), &next_bar(), None)
        .unwrap_err();
    assert_eq!(refused.code, daemon_refused.code);
    assert_eq!(refused.message, daemon_refused.message);
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        220.0.into()
    );
}

#[test]
fn rendering_lands_it_and_its_writes_without_allocating() {
    let mut engine = engine();
    // The first block compiles the graph.
    render(&mut engine, 1);
    let queued = engine
        .apply_edits_at(&edits(WRITES), &next_bar(), None)
        .unwrap();
    engine.write_controls(&[author(440.0)]).unwrap();
    let mut out = vec![0.0f32; 2 * BLOCK * AFTER];
    let (_, allocs, frees) = allocator_events(|| engine.render_interleaved(&mut out).unwrap());
    assert_eq!((allocs, frees), (0, 0));
    let adopted = engine.request_status(&queued.id).unwrap();
    assert_eq!(adopted.state, RequestState::Adopted);
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        440.0.into()
    );
}
