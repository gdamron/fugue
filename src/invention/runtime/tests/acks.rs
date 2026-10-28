//! Control writes that tell their caller what became of each: applied
//! within a few device blocks, pending past them and announced naming
//! their request once applied, never announced when a later write at the
//! same sample replaced them.

use super::*;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::invention::publish::Progress;
use crate::rpc::{ControlRequestState, ControlTiming, ControlWhen, RequestSource};
use crate::{ControlWrite, ControlWriteIntent, RpcEventPayload};
use std::sync::Arc;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn runtime() -> InventionRuntime {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    InventionBuilder::new(48_000).build(invention).unwrap().0
}

pub(super) fn frequency(value: f32, source: RequestSource) -> ControlWrite {
    ControlWrite {
        intent: ControlWriteIntent::Perform,
        source: Some(source),
        ..ControlWrite::new("osc", "frequency", value.into())
    }
}

/// Each `ControlChanged` announced, with the request it names.
fn announced(sink: &RecordingSink) -> Vec<(ControlValue, Option<String>)> {
    let events = sink.events.lock().unwrap();
    (events.iter())
        .filter_map(|event| match event {
            RpcEventPayload::ControlChanged { value, request, .. } => {
                Some((value.clone(), request.clone()))
            }
            _ => None,
        })
        .collect()
}

pub(super) fn on_manual() -> (RunningInvention, Pump, Arc<RecordingSink>) {
    let (running, pump) = start_manual(runtime());
    let sink = Arc::new(RecordingSink::default());
    running.set_event_sink(sink.clone());
    (running, pump, sink)
}

#[test]
fn a_write_applied_within_the_wait_is_acknowledged_applied_and_announced() {
    // Blocks reported long, so the reply waits its longest, 200 ms, for a
    // block every 2 ms to take the write.
    let backend = TickBackend::new(48_000).reporting_period(Duration::from_millis(50));
    let running = runtime().start_with_backend(backend).unwrap();
    // The device has played a block, so the reply waits for its writes.
    assert!(wait_until(|| running
        .status()
        .diagnostics
        .is_some_and(|diagnostics| diagnostics.callback_count > 0)));
    let sink = Arc::new(RecordingSink::default());
    running.set_event_sink(sink.clone());

    let acks = running.write_controls(&[frequency(330.0, RequestSource::Agent)]);
    running.stop();

    assert!(acks.error.is_none());
    let [ack] = &acks.controls[..] else {
        panic!("one write, one ack: {:?}", acks.controls);
    };
    assert_eq!(ack.state, Some(ControlRequestState::Applied));
    assert_eq!(ack.value, Some(330.0.into()));
    assert_eq!(ack.requested, Some(330.0.into()));
    assert!(ack.request.is_some());
    assert_eq!(announced(&sink), [(330.0.into(), ack.request.clone())]);
}

#[test]
fn a_write_no_device_has_taken_is_pending_and_announced_once_applied() {
    let (running, pump, sink) = on_manual();

    // No device plays on its own, so the reply does not wait.
    let acks = running.write_controls(&[frequency(330.0, RequestSource::User)]);
    let [ack] = &acks.controls[..] else {
        panic!("one write, one ack: {:?}", acks.controls);
    };
    assert_eq!(ack.state, Some(ControlRequestState::Pending));
    assert_eq!(ack.value, None);
    assert_eq!(ack.requested, Some(330.0.into()));
    assert!(announced(&sink).is_empty());

    pump.block();
    running.announce_applied();
    assert_eq!(announced(&sink), [(330.0.into(), ack.request.clone())]);
    // Announced once.
    running.announce_applied();
    assert_eq!(announced(&sink).len(), 1);
    // Its reply sent, nothing holds its outcome.
    assert_eq!(running.live.held(), 0);
}

#[test]
fn a_pending_write_a_later_one_replaces_at_its_sample_is_never_announced() {
    let (running, pump, sink) = on_manual();

    let acks = running.write_controls(&[
        frequency(330.0, RequestSource::User),
        frequency(440.0, RequestSource::Agent),
    ]);
    let states: Vec<_> = acks.controls.iter().map(|ack| ack.state).collect();
    assert_eq!(states, [Some(ControlRequestState::Pending); 2]);

    pump.block();
    running.announce_applied();
    // A user's and an agent's rank alike, so the later wins.
    assert_eq!(
        running.get_control("osc", "frequency").unwrap(),
        440.0.into()
    );
    let second = acks.controls[1].request.clone();
    assert_eq!(announced(&sink), [(440.0.into(), second)]);
}

#[test]
fn a_write_that_cannot_be_made_stops_the_batch_and_those_before_it_stand() {
    let (running, pump, _sink) = on_manual();

    let acks = running.write_controls(&[
        frequency(330.0, RequestSource::User),
        ControlWrite::new("gone", "frequency", 440.0.into()),
        frequency(550.0, RequestSource::User),
    ]);

    assert!(matches!(
        acks.error,
        Some(GraphCommandError::UnknownModule(ref id)) if id == "gone"
    ));
    assert_eq!(acks.controls.len(), 1);
    pump.block();
    assert_eq!(
        running.get_control("osc", "frequency").unwrap(),
        330.0.into()
    );
}

#[test]
fn a_schedule_travelling_in_a_publication_is_pending_until_it_installs() {
    let document = r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 97.0 } },
            { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
                { "at_step": 2, "module": "osc", "control": "frequency", "value": 330.0 }
            ] } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" },
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let invention = Invention::from_json(document).unwrap();
    let runtime = InventionBuilder::new(48_000).build(invention).unwrap().0;
    let (running, pump) = start_manual(runtime);
    let generation = running.live.generation();

    // Retargeted onto the clock's tempo, it is recompiled and published.
    let tempo = r#"[{ "at_step": 2, "module": "clock", "control": "bpm", "value": 211.0 }]"#;
    let acks = running.write_controls(&[ControlWrite::new("sched", "schedule", tempo.into())]);

    assert_eq!(running.live.generation(), generation + 1);
    let [ack] = &acks.controls[..] else {
        panic!("one write, one ack: {:?}", acks.controls);
    };
    assert_eq!(ack.state, Some(ControlRequestState::Pending));
    assert_eq!(ack.request, None);
    pump.block();
    assert!(running.live.installed(generation + 1));
}

#[test]
fn a_write_applied_before_an_edit_batch_is_announced_before_the_batchs() {
    let (mut running, pump, sink) = on_manual();
    let acks = running.write_controls(&[frequency(330.0, RequestSource::User)]);
    // Applied, and not yet announced, as the batch commits.
    pump.block();

    let edit = crate::rpc::StructuralEdit::SetControl {
        module_id: "osc".into(),
        key: "frequency".into(),
        value: 440.0.into(),
    };
    running.apply_edits(&[edit]).expect("the batch commits");

    let first = acks.controls[0].request.clone();
    assert_eq!(
        announced(&sink),
        [(330.0.into(), first), (440.0.into(), None)]
    );
}

#[test]
fn a_batch_wider_than_the_shared_outcomes_says_what_became_of_each() {
    // Each write settles before the next is made, all at one sample, so
    // the first, ranked highest, holds the control against the rest: made
    // now, or timed to a sample passed already.
    for when in [None, Some(ControlWhen::AtSample { sample: 0 })] {
        let backend = crate::modules::NullBackend::new(48_000);
        let running = runtime().start_with_backend(backend).unwrap();
        let timing = ControlTiming { when, ttl_ms: None };
        let write = |value: f32| ControlWrite {
            timing: timing.clone(),
            ..frequency(value, RequestSource::User)
        };
        let ranked = ControlWrite {
            priority: Some(1),
            ..write(330.0)
        };
        let wider = crate::invention::publish::OUTCOME_QUEUE_CAPACITY + 1;
        let rest = (0..wider).map(|step| write(400.0 + step as f32));
        let writes: Vec<_> = std::iter::once(ranked).chain(rest).collect();

        let acks = running.write_controls(&writes);

        assert!(acks.error.is_none());
        assert_eq!(acks.controls[0].state, Some(ControlRequestState::Applied));
        let lost = (acks.controls[1..].iter())
            .filter(|ack| ack.state != Some(ControlRequestState::Applied));
        assert_eq!(lost.count(), wider, "none of the rest applied: {timing:?}");
        assert_eq!(
            running.get_control("osc", "frequency").unwrap(),
            330.0.into()
        );
        running.stop();
    }
}

/// Queues an edit adding an lfo and retuning osc, landing 256 samples on.
pub(super) fn retune_waiting(running: &mut RunningInvention) {
    let lfo = crate::rpc::StructuralEdit::AddModule {
        id: "lfo".into(),
        module_type: "oscillator".into(),
        config: serde_json::json!({ "frequency": 3.0 }),
    };
    let retune = crate::rpc::StructuralEdit::SetControl {
        module_id: "osc".into(),
        key: "frequency".into(),
        value: 550.0.into(),
    };
    let when = crate::rpc::EditWhen::AfterSamples { samples: 256 };
    (running.apply_edits_at(&[lfo, retune], &when, None)).expect("the edit is queued");
}

/// `written`'s request id.
fn request_id(written: &Option<String>) -> crate::control_request::RequestId {
    crate::control_request::RequestId(written.as_deref().unwrap().parse().unwrap())
}

/// osc's frequency as the document records it.
fn recorded(running: &RunningInvention) -> Option<f64> {
    let document = running.document().unwrap();
    let osc = document.modules.iter().find(|module| module.id == "osc");
    osc.unwrap().config["frequency"].as_f64()
}

#[test]
fn a_write_kept_over_a_timed_edit_is_announced_as_it_applies_and_plays_on_past_it() {
    let (mut running, pump, sink) = on_manual();
    retune_waiting(&mut running);
    let authored = ControlWrite::new("osc", "frequency", 330.0.into());
    let acks = running.write_controls(&[authored]);
    let written = acks.controls[0].request.clone();

    pump.block();
    running.announce_applied();
    assert!(matches!(
        running.live.progress(request_id(&written)),
        Progress::Settled(crate::control_request::Outcome::Applied { .. })
    ));
    assert_eq!(announced(&sink), [(330.0.into(), written.clone())]);

    // The edit lands, its retune followed by the kept write's copy.
    pump.render(8);
    running.live.adopt_landed();
    running.announce_applied();
    assert_eq!(
        running.get_control("osc", "frequency").unwrap(),
        330.0.into()
    );
    assert_eq!(recorded(&running), Some(330.0));
    // Announced once, as the write applied; its copy is the edit's.
    assert_eq!(announced(&sink), [(330.0.into(), written)]);
}

/// A kept write a gesture replaces at its exact sample is superseded, as
/// its reply says, and its copy, which waits on it applying, breaks: as
/// the edit lands its own value plays and is recorded, as it would were
/// nothing kept. Nothing more is announced.
#[test]
fn a_kept_write_replaced_at_its_sample_is_superseded_and_not_reasserted() {
    let (mut running, pump, sink) = on_manual();
    retune_waiting(&mut running);
    let authored = ControlWrite::new("osc", "frequency", 330.0.into());
    let acks = running.write_controls(&[authored, frequency(440.0, RequestSource::User)]);
    let (written, gesture) = (
        acks.controls[0].request.clone(),
        acks.controls[1].request.clone(),
    );

    pump.block();
    running.announce_applied();
    let superseded = Progress::Settled(crate::control_request::Outcome::Superseded);
    assert_eq!(running.live.progress(request_id(&written)), superseded);
    assert_eq!(announced(&sink), [(440.0.into(), gesture.clone())]);

    pump.render(8);
    running.live.adopt_landed();
    running.announce_applied();
    assert_eq!(
        running.get_control("osc", "frequency").unwrap(),
        550.0.into()
    );
    assert_eq!(recorded(&running), Some(550.0));
    assert_eq!(announced(&sink), [(440.0.into(), gesture)]);
}

#[test]
fn a_kept_writes_copy_holds_no_outcome_once_its_reply_is_sent() {
    let (mut running, pump, _sink) = on_manual();
    retune_waiting(&mut running);
    let authored = ControlWrite::new("osc", "frequency", 330.0.into());
    running.write_controls(&[authored]);

    // Its copy, unawaited, settles as the edit lands, long after the reply.
    pump.render(8);
    running.announce_applied();
    assert_eq!(running.live.held(), 0);
}

#[test]
fn an_authored_write_pending_at_its_reply_moves_the_authored_epoch_once_it_applies() {
    let (running, pump, _sink) = on_manual();
    let epoch = running.authored_epoch();
    let authored = ControlWrite::new("osc", "frequency", 330.0.into());
    let acks = running.write_controls(&[authored]);
    assert_eq!(acks.controls[0].state, Some(ControlRequestState::Pending));
    assert_eq!(running.authored_epoch(), epoch);

    pump.block();
    assert_eq!(running.authored_epoch(), epoch + 1);
    pump.render(4);
    assert_eq!(running.authored_epoch(), epoch + 1);
    // A performed write is never recorded.
    running.write_controls(&[frequency(440.0, RequestSource::User)]);
    pump.render(2);
    assert_eq!(running.authored_epoch(), epoch + 1);
}

/// A document read with its epoch holds every write the epoch counts: the
/// one applied since the last read is recorded and counted together.
#[test]
fn a_document_read_with_its_epoch_holds_every_write_counted() {
    let (running, pump, _sink) = on_manual();
    let frequency_in = |document: Option<Invention>| {
        let document = document.unwrap();
        let osc = document.modules.iter().find(|spec| spec.id == "osc");
        osc.unwrap().config["frequency"].as_f64()
    };
    let (document, epoch) = running.document_with_epoch();
    assert_eq!(frequency_in(document), Some(220.0));

    let authored = ControlWrite::new("osc", "frequency", 330.0.into());
    running.write_controls(&[authored]);
    let (document, unmoved) = running.document_with_epoch();
    assert_eq!((frequency_in(document), unmoved), (Some(220.0), epoch));

    // Applied, and recorded only as the read takes it up.
    pump.block();
    let (document, moved) = running.document_with_epoch();
    assert_eq!((frequency_in(document), moved), (Some(330.0), epoch + 1));
}
