//! A timed edit batch on an offline render meets what else happens while
//! it waits or has just landed as the daemon's does: a load right after it
//! lands, a write due on its sample, refusals with one already waiting, a
//! ttl running out, a schedule write racing it, and many cancelled in a
//! row.

use super::timed_edit_tests::{daemon, edits, next_bar, unnumbered, ADD, SWAP};
use super::timed_write_tests::{daemon_render, engine, render, AFTER, BLOCK, DOCUMENT, LEAD_IN};
use super::RenderEngine;
use crate::rpc::{ControlTiming, ControlWhen, EditWhen, RequestState};
use crate::{ControlValue, ControlWrite, ControlWriteIntent};

fn frequency(intent: ControlWriteIntent, when: Option<ControlWhen>) -> ControlWrite {
    ControlWrite {
        intent,
        timing: ControlTiming { when, ttl_ms: None },
        ..ControlWrite::new("osc", "frequency", ControlValue::Number(330.0))
    }
}

/// The frequency the document records for `osc`.
fn recorded(document: Option<crate::Invention>) -> Option<f64> {
    let document = document.unwrap();
    let osc = document.modules.iter().find(|m| m.id == "osc").unwrap();
    osc.config["frequency"].as_f64()
}

/// A load right after a render landed the edit, with no call between,
/// adopts it first: it reads adopted, and the piece loaded plays and
/// takes writes as a fresh one does, refusing one to the module the edit
/// added.
#[test]
fn a_load_right_after_it_lands_adopts_it_and_the_new_piece_takes_writes() {
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    let queued = engine
        .apply_edits_at(&edits(ADD), &next_bar(), None)
        .unwrap();
    render(&mut engine, AFTER);
    engine.load_json(DOCUMENT).unwrap();
    let changes = engine.take_request_changes();
    let last = changes.iter().rev().find(|c| c.id == queued.id).unwrap();
    assert_eq!(last.state, RequestState::Adopted, "{changes:?}");

    let mut fresh = self::engine();
    let write = frequency(ControlWriteIntent::Perform, None);
    engine.write_controls(std::slice::from_ref(&write)).unwrap();
    fresh.write_controls(&[write]).unwrap();
    assert_eq!(render(&mut engine, 50), render(&mut fresh, 50));
    let added = ControlWrite::new("low", "frequency", ControlValue::Number(55.0));
    let refused = |engine: &mut RenderEngine| {
        let written = engine.write_controls(std::slice::from_ref(&added));
        format!("{written:?}")
    };
    assert_eq!(refused(&mut engine), refused(&mut fresh));
    assert!(engine.get_control("low", "frequency").is_err());
}

/// An authored write due on the sample the edit lands on, to the module
/// it rebuilds, leaves the document as the daemon's leaves it.
#[test]
fn a_write_due_on_its_sample_to_a_module_it_rebuilds_is_recorded_as_the_daemons() {
    let (mut running, pump) = daemon();
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    daemon_render(&pump, LEAD_IN);
    let lands = (LEAD_IN * BLOCK) as u64 + 1000;
    let when = EditWhen::AtSample { sample: lands };
    let daemon_queued = running.apply_edits_at(&edits(SWAP), &when, None).unwrap();
    let queued = engine.apply_edits_at(&edits(SWAP), &when, None).unwrap();
    let write = frequency(
        ControlWriteIntent::Author,
        Some(ControlWhen::AtSample { sample: lands }),
    );
    running.write_controls(std::slice::from_ref(&write));
    engine.write_controls(&[write]).unwrap();
    assert_eq!(render(&mut engine, AFTER), daemon_render(&pump, AFTER));
    let status = engine.request_status(&queued.id).unwrap();
    let daemon_status = running.request_status(&daemon_queued.id).unwrap();
    assert_eq!(unnumbered(status), unnumbered(daemon_status));
    assert_eq!(recorded(engine.document()), recorded(running.document()));
}

/// With one edit waiting, a batch naming a time that can never come is
/// refused for its time, as the daemon refuses it, before it is refused
/// for waiting.
#[test]
fn with_one_waiting_a_bad_time_is_refused_as_the_daemon_refuses_it() {
    let (mut running, _pump) = daemon();
    let engine = engine();
    running
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    let unclocked = EditWhen::NextBar {
        beats_per_bar: None,
        clock: Some("osc".into()),
    };
    let negative = EditWhen::AfterBeats {
        beats: -1.0,
        clock: None,
    };
    for when in [unclocked, negative, next_bar()] {
        let daemon = running
            .apply_edits_at(&edits(ADD), &when, None)
            .unwrap_err();
        let error = engine.apply_edits_at(&edits(ADD), &when, None).unwrap_err();
        assert_eq!(error.code, daemon.code, "{when:?}");
        assert_eq!(error.message, daemon.message, "{when:?}");
    }
}

/// One whose ttl runs out before its time expires as the daemon's does,
/// and never plays.
#[test]
fn one_whose_ttl_runs_out_expires_as_the_daemons_does() {
    let (mut running, pump) = daemon();
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    daemon_render(&pump, LEAD_IN);
    let when = EditWhen::AtSample {
        sample: (LEAD_IN * BLOCK) as u64 + 12_000,
    };
    let daemon_queued = running
        .apply_edits_at(&edits(SWAP), &when, Some(1))
        .unwrap();
    let queued = engine.apply_edits_at(&edits(SWAP), &when, Some(1)).unwrap();
    assert_eq!(render(&mut engine, AFTER), daemon_render(&pump, AFTER));
    let status = engine.request_status(&queued.id).unwrap();
    let daemon_status = running.request_status(&daemon_queued.id).unwrap();
    assert_eq!(status.state, RequestState::Expired, "{status:?}");
    assert_eq!(unnumbered(status), unnumbered(daemon_status));
}

/// Many cancelled in a row each leave room for the next, which still
/// queues: none holds the queue until it would have been due.
#[test]
fn edits_cancelled_in_a_row_each_give_their_room_back() {
    let mut engine = engine();
    render(&mut engine, LEAD_IN);
    let author = frequency(ControlWriteIntent::Author, None);
    for round in 0..100 {
        let queued = engine.apply_edits_at(&edits(SWAP), &next_bar(), None);
        let queued = queued.unwrap_or_else(|error| panic!("round {round}: {error:?}"));
        engine
            .write_controls(std::slice::from_ref(&author))
            .unwrap();
        let status = engine.request_status(&queued.id).unwrap();
        assert_eq!(status.state, RequestState::Cancelled, "round {round}");
        render(&mut engine, 1);
    }
}

/// A clock stepping a scheduler that retunes the oscillator, as `DOCUMENT`
/// plays it otherwise.
const SCHEDULED: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 480.0 } },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
            { "at_step": 12, "module": "osc", "control": "frequency", "value": 330.0 }
        ] } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" },
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// A schedule written to a scheduler the waiting edit re-resolves cancels
/// it, as on the daemon, and the new schedule plays.
#[test]
fn a_schedule_written_while_it_waits_cancels_it_as_the_daemons_is() {
    let invention = crate::Invention::from_json(SCHEDULED).unwrap();
    let rate = super::timed_write_tests::RATE;
    let (runtime, _) = crate::InventionBuilder::new(rate).build(invention).unwrap();
    let (mut running, pump) = crate::invention::manual_backend::start_manual(runtime);
    let mut engine = RenderEngine::new(rate);
    engine.set_block_size(BLOCK);
    engine.load_json(SCHEDULED).unwrap();
    render(&mut engine, LEAD_IN);
    daemon_render(&pump, LEAD_IN);

    let daemon_queued = running
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    let queued = engine
        .apply_edits_at(&edits(SWAP), &next_bar(), None)
        .unwrap();
    let schedule = r#"[{"at_step":10,"module":"osc","control":"frequency","value":440.0}]"#;
    let schedule = ControlValue::String(schedule.into());
    running
        .set_control("sched", "schedule", schedule.clone())
        .unwrap();
    engine.set_control("sched", "schedule", schedule).unwrap();
    let status = engine.request_status(&queued.id).unwrap();
    assert_eq!(status.state, RequestState::Cancelled, "{status:?}");
    let daemon_status = running.request_status(&daemon_queued.id).unwrap();
    assert_eq!(unnumbered(status), unnumbered(daemon_status));
    assert_eq!(render(&mut engine, AFTER), daemon_render(&pump, AFTER));
}
