//! Timed control writes through the runtime's front door, in the wire's
//! terms: a write on the next downbeat is acknowledged with its beat,
//! listed pending, read back only once it lands, and announced settled; one
//! some samples on is acknowledged with the sample it lands on; one
//! after another request lands that many samples after it, whether it is
//! still waiting, applied already or a timed edit; one whose ttl passes
//! first expires and never applies; one waiting as the invention stops is
//! announced refused; one on a sample past as many as may wait is refused,
//! so writes far ahead never hold up one made now; and a time that can
//! never come, or a ttl without a time, is refused with nothing queued.

use super::timed::{downbeat, BAR};
use super::*;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::invention::publish::timed_write::SAMPLE_TIMED_CAPACITY;
use crate::rpc::{
    ControlRequestSettled, ControlRequestState, ControlTiming, ControlWhen, EditWhen,
    PendingDetail, PendingWait, RequestSource,
};
use crate::{ControlWrite, ControlWriteIntent, LoggedBeat, RpcEventPayload};

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 480.0 } },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "tune", "type": "melody", "config": { "degrees": [0, 2, 4] } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn start() -> (RunningInvention, Pump, Arc<RecordingSink>) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let sink = Arc::new(RecordingSink::default());
    running.set_event_sink(sink.clone());
    (running, pump, sink)
}

/// An agent's write of `key` of `module_id`, at `when`.
fn timed(module_id: &str, key: &str, value: ControlValue, when: ControlWhen) -> ControlWrite {
    ControlWrite {
        intent: ControlWriteIntent::Perform,
        source: Some(RequestSource::Agent),
        timing: ControlTiming {
            when: Some(when),
            ttl_ms: None,
        },
        ..ControlWrite::new(module_id, key, value)
    }
}

fn next_bar() -> ControlWhen {
    ControlWhen::NextBar {
        beats_per_bar: 4.0,
        clock: None,
    }
}

/// Makes `write` and returns the request its reply names.
fn submit(running: &RunningInvention, write: ControlWrite) -> crate::rpc::AppliedControl {
    let acks = running.write_controls(&[write]);
    assert!(acks.error.is_none(), "{:?}", acks.error);
    acks.controls.into_iter().next().unwrap()
}

/// Each timed write settled, in the order announced.
fn settled(sink: &RecordingSink) -> Vec<ControlRequestSettled> {
    let events = sink.events.lock().unwrap();
    (events.iter())
        .filter_map(|event| match event {
            RpcEventPayload::ControlRequestSettled(settled) => Some(settled.clone()),
            _ => None,
        })
        .collect()
}

fn frequency(running: &RunningInvention) -> ControlValue {
    running.get_control("osc", "frequency").unwrap()
}

#[test]
fn a_write_on_the_next_downbeat_is_listed_then_read_back_once_it_lands() {
    let (running, pump, sink) = start();
    pump.render(100);
    let ack = submit(
        &running,
        timed("osc", "frequency", 330.0.into(), next_bar()),
    );
    assert_eq!(ack.state, Some(ControlRequestState::Pending));
    assert_eq!(ack.value, None, "not read back before it lands");
    let beat = LoggedBeat::Grid {
        clock: "clock".into(),
        every: 4.0,
        offset: 0.0,
        repeat: false,
    };
    let landing = ack.landing.clone().unwrap();
    assert_eq!(landing.beat, Some(beat.clone()), "on the first clock");

    let listed = running.pending_requests(None, PendingDetail::Full);
    let [item] = &listed.items[..] else {
        panic!("{listed:?}");
    };
    assert_eq!(Some(&item.request), ack.request.as_ref());
    assert_eq!(item.waiting, PendingWait::Beat);
    assert_eq!(item.landing.as_ref().unwrap().beat, Some(beat));

    // Just short of the downbeat, nothing has changed.
    let downbeat = downbeat(4.0);
    pump.render(downbeat / 64 - 100);
    running.announce_applied();
    assert_eq!(frequency(&running), 220.0.into());
    assert!(settled(&sink).is_empty());

    pump.render(4);
    running.announce_applied();
    assert_eq!(frequency(&running), 330.0.into());
    assert!(running
        .pending_requests(None, PendingDetail::Full)
        .items
        .is_empty());
    let [landed] = &settled(&sink)[..] else {
        panic!("settled once");
    };
    assert_eq!(landed.state, ControlRequestState::Applied);
    assert_eq!(Some(&landed.request), ack.request.as_ref());
    assert_eq!(landed.landing.sample, Some(downbeat as u64));
    // Its change announced too, naming it.
    let events = sink.events.lock().unwrap();
    assert!(events.iter().any(|event| matches!(event,
        RpcEventPayload::ControlChanged { request, .. } if *request == ack.request)));
}

#[test]
fn a_write_after_another_request_lands_that_many_samples_after_it() {
    let (running, pump, sink) = start();
    let first = submit(&running, timed("osc", "frequency", 330.0.into(), at(1000)));
    let after = |request: &crate::rpc::AppliedControl, offset| ControlWhen::AfterRequest {
        request: request.request.clone().unwrap(),
        offset_samples: offset,
    };
    // Waiting on one still waiting: counted from where it applies.
    let second = submit(
        &running,
        timed("osc", "frequency", 440.0.into(), after(&first, 100)),
    );
    assert_eq!(second.state, Some(ControlRequestState::Pending));
    pump.render(40);
    running.announce_applied();
    let landed: Vec<_> = settled(&sink).iter().map(|s| s.landing.sample).collect();
    assert_eq!(landed, [Some(1000), Some(1100)]);
    assert_eq!(frequency(&running), 440.0.into());

    // After one applied already: its sample is known up front.
    let third = submit(
        &running,
        timed("osc", "frequency", 550.0.into(), after(&second, 64)),
    );
    assert_eq!(
        third.landing.as_ref().unwrap().sample,
        Some(1164),
        "{third:?}"
    );

    // One that never applied, or no request at all, has nothing to follow.
    let refused = running.write_controls(&[timed(
        "osc",
        "frequency",
        1.0.into(),
        ControlWhen::AfterRequest {
            request: "999999".into(),
            offset_samples: 0,
        },
    )]);
    let error = refused.error.unwrap().to_string();
    assert!(error.contains("no request 999999"), "{error}");
}

#[test]
fn a_write_after_a_timed_edit_lands_on_the_edit_s_sample() {
    let (running, pump, sink) = start();
    pump.render(100);
    let config = serde_json::json!({ "degrees": [0, 7] });
    let edit_when = EditWhen::NextBar {
        beats_per_bar: 4.0,
        clock: None,
    };
    let edit = running
        .swap_module_at("tune", "melody", &config, true, &edit_when, None)
        .unwrap();
    let when = ControlWhen::AfterRequest {
        request: edit.id.clone(),
        offset_samples: 0,
    };
    let ack = submit(&running, timed("osc", "frequency", 330.0.into(), when));
    assert_eq!(ack.state, Some(ControlRequestState::Pending));
    pump.render(BAR / 64 + 4);
    running.announce_applied();
    let [landed] = &settled(&sink)[..] else {
        panic!("settled once");
    };
    assert_eq!(landed.landing.sample, Some(downbeat(4.0) as u64));
    assert_eq!(frequency(&running), 330.0.into());
}

#[test]
fn a_write_whose_ttl_passes_first_expires_and_never_applies() {
    let (running, pump, sink) = start();
    pump.render(100);
    let write = ControlWrite {
        timing: ControlTiming {
            when: Some(next_bar()),
            ttl_ms: Some(10),
        },
        ..timed("osc", "frequency", 330.0.into(), next_bar())
    };
    let ack = submit(&running, write);
    pump.render(BAR / 64 + 4);
    running.announce_applied();
    assert_eq!(frequency(&running), 220.0.into(), "never applied");
    let [expired] = &settled(&sink)[..] else {
        panic!("settled once");
    };
    assert_eq!(Some(&expired.request), ack.request.as_ref());
    assert_eq!(expired.state, ControlRequestState::Expired);
    assert!(expired.reason.as_ref().unwrap().contains("ttl"));
    assert_eq!(expired.landing.sample, None);
    let events = sink.events.lock().unwrap();
    let changed =
        (events.iter()).any(|event| matches!(event, RpcEventPayload::ControlChanged { .. }));
    assert!(!changed, "no change announced");
}

fn at(sample: u64) -> ControlWhen {
    ControlWhen::AtSample { sample }
}

#[test]
fn a_timed_write_refused_where_it_cannot_wait_says_why() {
    let (running, _pump, _sink) = start();
    let when = ControlWhen::NextBeat {
        every: 0.0,
        offset: 0.0,
        clock: None,
    };
    let refused = running.write_controls(&[timed("osc", "frequency", 1.0.into(), when)]);
    let error = refused.error.unwrap().to_string();
    assert!(error.contains("positive `every`"), "{error}");
    let not_a_clock = ControlWhen::AfterBeats {
        beats: 1.0,
        clock: Some("osc".into()),
    };
    let refused = running.write_controls(&[timed("osc", "frequency", 1.0.into(), not_a_clock)]);
    assert!(refused.error.is_some());
    // A ttl bounds only a timed write: alone it is refused, as MCP refuses it.
    let mut bounded = ControlWrite::new("osc", "frequency", 1.0.into());
    bounded.timing.ttl_ms = Some(10);
    let refused = running.write_controls(&[bounded]);
    let error = refused.error.unwrap().to_string();
    assert!(error.contains("only with a `when`"), "{error}");
    assert!(running
        .pending_requests(None, PendingDetail::Full)
        .items
        .is_empty());
}

#[test]
fn a_timed_write_waiting_as_the_invention_stops_is_announced_refused() {
    let (running, pump, sink) = start();
    pump.render(4);
    let ack = submit(
        &running,
        timed("osc", "frequency", 330.0.into(), next_bar()),
    );
    let untimed = submit(&running, ControlWrite::new("tune", "degree.1", 9.0.into()));
    running.stop();
    let [refused] = &settled(&sink)[..] else {
        panic!("settled once, the untimed write unannounced");
    };
    assert_eq!(Some(&refused.request), ack.request.as_ref());
    assert_ne!(untimed.request, ack.request);
    assert_eq!(refused.state, ControlRequestState::Refused);
    let reason = refused.reason.as_deref().unwrap();
    assert_eq!(reason, "the invention it was queued on stopped");
}

#[test]
fn writes_far_ahead_on_a_sample_never_hold_up_one_made_now() {
    let (running, pump, _sink) = start();
    let far = |n: usize| {
        let value = (300.0 + n as f32).into();
        timed("osc", "frequency", value, at(1_000_000_000 + n as u64))
    };
    let all: Vec<usize> = (0..SAMPLE_TIMED_CAPACITY).collect();
    for batch in all.chunks(64) {
        let acks = running.write_controls(&batch.iter().map(|&n| far(n)).collect::<Vec<_>>());
        assert!(acks.error.is_none(), "{:?}", acks.error);
        pump.render(1);
    }
    let refused = running.write_controls(&[far(SAMPLE_TIMED_CAPACITY)]);
    let error = refused.error.unwrap().to_string();
    assert!(error.contains("wait on a sample"), "{error}");

    submit(&running, ControlWrite::new("tune", "degree.1", 9.0.into()));
    pump.render(4);
    assert_eq!(running.get_control("tune", "degree.1").unwrap(), 9.0.into());
    assert_eq!(frequency(&running), 220.0.into(), "none far ahead applied");
}

#[test]
fn a_write_some_samples_on_is_acknowledged_with_the_sample_it_lands_on() {
    let (running, pump, sink) = start();
    pump.render(4);
    let when = ControlWhen::AfterSamples { samples: 4800 };
    let ack = submit(&running, timed("osc", "frequency", 330.0.into(), when));
    let due = ack
        .landing
        .unwrap()
        .sample
        .expect("the sample it is due on");
    assert!(due >= 4800, "{due}");
    pump.render(4800 / 64 + 4);
    running.announce_applied();
    let [landed] = &settled(&sink)[..] else {
        panic!("settled once");
    };
    assert_eq!(landed.landing.sample, Some(due));
}
