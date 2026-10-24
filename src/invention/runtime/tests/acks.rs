//! Control writes that tell their caller what became of each: applied
//! within a few device blocks, pending past them and announced naming
//! their request once applied, never announced when a later write at the
//! same sample replaced them.

use super::*;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::rpc::{ControlRequestState, RequestSource};
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

fn frequency(value: f32, source: RequestSource) -> ControlWrite {
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

fn on_manual() -> (RunningInvention, Pump, Arc<RecordingSink>) {
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
