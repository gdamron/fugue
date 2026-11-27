//! Timed edit batches on an offline render land where the daemon's do:
//! the same piece, tempo and batch render sample for sample alike, on a
//! sample, some samples on, or on a beat, and read back with the daemon's
//! status and landing. One waiting is cancelled by a later edit, by an
//! authored write to a module it rebuilds, or as the piece is replaced,
//! and kept through a pause; only one waits at a time; and the render
//! lands it without allocating.

use super::timed_write_tests::{
    beat_sample, daemon_render, engine, render, AFTER, BLOCK, DOCUMENT, LEAD_IN, RATE,
};
use super::RenderEngine;
use crate::alloc_counter::allocator_events;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::rpc::{EditWhen, RequestState, RequestStatus, StructuralEdit};
use crate::RunningInvention;
use crate::{ControlValue, ControlWrite, ControlWriteIntent, Invention, InventionBuilder};

pub(super) fn daemon() -> (RunningInvention, Pump) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
    start_manual(runtime)
}

pub(super) fn edits(json: &str) -> Vec<StructuralEdit> {
    serde_json::from_str(json).unwrap()
}

/// Swaps the sine for a square at the same pitch.
pub(super) const SWAP: &str = r#"[{"op":"swap_module","id":"osc","module_type":"oscillator",
    "config":{"waveform":"square","frequency":220.0}}]"#;
/// Adds a second voice an octave down, wired to the output.
pub(super) const ADD: &str = r#"[
    {"op":"add_module","id":"low","module_type":"oscillator","config":{"frequency":110.0}},
    {"op":"connect","from":"low","from_port":"audio","to":"dac","to_port":"audio"}
]"#;

pub(super) fn next_bar() -> EditWhen {
    EditWhen::NextBar {
        beats_per_bar: Some(4.0),
        clock: None,
    }
}

/// Each kind of time, and the sample it lands on where that is plain.
pub(super) fn whens() -> [(EditWhen, Option<u64>); 5] {
    let now = (LEAD_IN * BLOCK) as u64;
    [
        (EditWhen::AtSample { sample: now + 1000 }, Some(now + 1000)),
        (EditWhen::AfterSamples { samples: 777 }, None),
        (
            EditWhen::AfterBeats {
                beats: 1.0,
                clock: None,
            },
            None,
        ),
        (
            EditWhen::NextBeat {
                every: 2.0,
                offset: 0.0,
                clock: Some("clock".into()),
            },
            Some(beat_sample(2.0)),
        ),
        (next_bar(), Some(beat_sample(4.0))),
    ]
}

/// The status without its id: the daemon also numbers its writes.
pub(super) fn unnumbered(status: RequestStatus) -> RequestStatus {
    RequestStatus {
        id: String::new(),
        ..status
    }
}

#[test]
fn a_timed_batch_lands_on_the_daemons_sample_with_its_status() {
    for batch in [SWAP, ADD] {
        for (when, lands) in whens() {
            let (mut running, pump) = daemon();
            let mut engine = engine();
            let lead_in = render(&mut engine, LEAD_IN);
            assert_eq!(lead_in, daemon_render(&pump, LEAD_IN), "{when:?}");

            let daemon_queued = running.apply_edits_at(&edits(batch), &when, None).unwrap();
            let queued = engine.apply_edits_at(&edits(batch), &when, None).unwrap();
            assert_eq!(queued.state, RequestState::Queued, "{when:?}");
            assert_eq!(
                unnumbered(queued.clone()),
                unnumbered(daemon_queued.clone())
            );

            let output = render(&mut engine, AFTER);
            assert_eq!(output, daemon_render(&pump, AFTER), "{batch} {when:?}");
            let daemon_adopted = running.request_status(&daemon_queued.id).unwrap();
            let adopted = engine.request_status(&queued.id).unwrap();
            assert_eq!(adopted.state, RequestState::Adopted, "{when:?}");
            assert_eq!(unnumbered(adopted.clone()), unnumbered(daemon_adopted));
            let landed = adopted.landing.sample.expect("landed on a sample");
            assert_eq!(lands.unwrap_or(landed), landed, "{when:?}");

            // Silent until it lands, then audibly the edit.
            let mut twin = self::engine();
            render(&mut twin, LEAD_IN);
            let unedited = render(&mut twin, AFTER);
            let first = (0..output.len()).find(|&i| output[i] != unedited[i]);
            let first = first.map(|i| (LEAD_IN * BLOCK + i / 2) as u64);
            assert!(first.is_some_and(|first| first >= landed), "{when:?}");
            assert!(first.is_some_and(|first| first <= landed + 1), "{when:?}");
        }
    }
}

#[test]
fn its_status_moves_from_queued_to_adopted_and_the_document_follows() {
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    let queued = engine
        .apply_edits_at(&edits(ADD), &next_bar(), None)
        .unwrap();
    let ids = |engine: &RenderEngine| {
        let changes = engine.take_request_changes();
        changes
            .into_iter()
            .map(|c| (c.id, c.state))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&engine), [(queued.id.clone(), RequestState::Queued)]);
    render(&mut engine, 100);
    assert_eq!(ids(&engine), []);
    let has_low = |engine: &RenderEngine| {
        let document = engine.document().unwrap();
        document.modules.iter().any(|module| module.id == "low")
    };
    assert!(!has_low(&engine), "not before it lands");
    assert!(engine.get_control("low", "frequency").is_err());
    render(&mut engine, AFTER);
    assert_eq!(ids(&engine), [(queued.id.clone(), RequestState::Adopted)]);
    let adopted = engine.request_status(&queued.id).unwrap();
    assert_eq!(adopted.built, ["low"]);
    assert!(has_low(&engine));
    assert_eq!(
        engine.get_control("low", "frequency").unwrap(),
        110.0.into()
    );
    let connected = engine.list_connections();
    assert!(connected.iter().any(|c| c.from == "low" && c.to == "dac"));
}

#[test]
fn a_later_edit_cancels_it_and_it_never_plays() {
    let mut engine = engine();
    let mut twin = self::engine();
    render(&mut engine, LEAD_IN);
    render(&mut twin, LEAD_IN);
    let queued = engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    engine.apply_edits(&edits(ADD)).unwrap();
    twin.apply_edits(&edits(ADD)).unwrap();
    let cancelled = engine.request_status(&queued.id).unwrap();
    assert_eq!(cancelled.state, RequestState::Cancelled);
    assert!(cancelled.reason.is_some());
    assert_eq!(render(&mut engine, AFTER), render(&mut twin, AFTER));
    let status = engine.request_status(&queued.id).unwrap();
    assert_eq!(status.state, RequestState::Cancelled, "stays cancelled");
}

#[test]
fn only_one_waits_at_a_time() {
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    let error = engine
        .apply_edits_at(&edits(ADD), &next_bar(), None)
        .unwrap_err();
    assert!(error.message.contains("already waits"), "{}", error.message);
}

#[test]
fn a_timed_batch_is_refused_as_the_daemon_refuses_it() {
    let (mut running, _pump) = daemon();
    let engine = engine();
    let unknown = r#"[{"op":"remove_module","id":"nope"}]"#;
    let unclocked = EditWhen::NextBar {
        beats_per_bar: None,
        clock: Some("osc".into()),
    };
    let negative = EditWhen::AfterBeats {
        beats: -1.0,
        clock: None,
    };
    for (batch, when) in [(unknown, next_bar()), (SWAP, unclocked), (SWAP, negative)] {
        let daemon = running
            .apply_edits_at(&edits(batch), &when, None)
            .unwrap_err();
        let error = engine
            .apply_edits_at(&edits(batch), &when, None)
            .unwrap_err();
        assert_eq!(error.code, daemon.code, "{batch} {when:?}");
        assert_eq!(error.message, daemon.message, "{batch} {when:?}");
    }
    assert!(engine.take_request_changes().is_empty());
}

#[test]
fn an_authored_write_to_a_module_it_rebuilds_cancels_it_and_others_keep_it() {
    let author = |module_id: &str, key: &str, value: f32| ControlWrite {
        intent: ControlWriteIntent::Author,
        ..ControlWrite::new(module_id, key, ControlValue::Number(value))
    };
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    let queued = engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    engine
        .write_controls(&[author("clock", "bpm", 480.0)])
        .unwrap();
    let kept = engine.request_status(&queued.id).unwrap();
    assert_eq!(kept.state, RequestState::Queued);
    engine
        .write_controls(&[author("osc", "frequency", 330.0)])
        .unwrap();
    let cancelled = engine.request_status(&queued.id).unwrap();
    assert_eq!(cancelled.state, RequestState::Cancelled);
    let reason = cancelled.reason.unwrap();
    assert!(reason.contains("osc.frequency"), "{reason}");
}

#[test]
fn replacing_the_piece_cancels_it_and_a_pause_keeps_it() {
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    let queued = engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    engine.reset().unwrap();
    let changes = engine.take_request_changes();
    let [cancelled] = &changes[..] else {
        panic!("{changes:?}");
    };
    assert_eq!(cancelled.id, queued.id);
    assert_eq!(cancelled.state, RequestState::Cancelled);
    let mut fresh = self::engine();
    assert_eq!(render(&mut engine, AFTER), render(&mut fresh, AFTER));

    let (running, pump) = daemon();
    let mut running = running;
    let mut engine = self::engine();
    render(&mut engine, LEAD_IN);
    daemon_render(&pump, LEAD_IN);
    let daemon_queued = running
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    let queued = engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    engine.pause();
    running.pause();
    assert_eq!(render(&mut engine, 50), daemon_render(&pump, 50));
    let waiting = engine.request_status(&queued.id).unwrap();
    assert_eq!(waiting.state, RequestState::Queued);
    engine.resume();
    running.resume();
    assert_eq!(render(&mut engine, AFTER), daemon_render(&pump, AFTER));
    let adopted = engine.request_status(&queued.id).unwrap();
    let daemon_adopted = running.request_status(&daemon_queued.id).unwrap();
    assert_eq!(adopted.state, RequestState::Adopted);
    assert_eq!(adopted.landing, daemon_adopted.landing);
}

#[test]
fn rendering_lands_a_timed_batch_without_allocating() {
    for batch in [SWAP, ADD] {
        let mut engine = engine();
        // The first block compiles the graph.
        render(&mut engine, 1);
        let queued = engine
            .apply_edits_at(&edits(batch), &next_bar(), None)
            .unwrap();
        let mut out = vec![0.0f32; 2 * BLOCK * AFTER];
        let (_, allocs, frees) = allocator_events(|| engine.render_interleaved(&mut out).unwrap());
        assert_eq!((allocs, frees), (0, 0), "{batch}");
        let adopted = engine.request_status(&queued.id).unwrap();
        assert_eq!(adopted.state, RequestState::Adopted, "{batch}");
    }
}
