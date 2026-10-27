//! Timed control writes on the wire: a write without `when` reads and
//! writes exactly as before, an agent's timed write parses, and its reply,
//! listing and settled event say where it lands and what became of it.

use super::*;
use crate::{ControlValue, LoggedBeat};

fn command(json: serde_json::Value) -> Result<RpcCommand, serde_json::Error> {
    serde_json::from_value(json)
}

#[test]
fn an_untimed_write_reads_and_writes_as_before() {
    let json = r#"{"command":"set_control","module_id":"osc","key":"frequency","value":440.0,"intent":"author"}"#;
    let write: RpcCommand = serde_json::from_str(json).unwrap();
    let RpcCommand::SetControl { timing, .. } = &write else {
        panic!("{write:?}");
    };
    assert_eq!(timing, &ControlTiming::default());
    assert!(!timing.is_set());
    assert_eq!(serde_json::to_string(&write).unwrap(), json);

    let batch = r#"{"command":"set_controls","writes":[{"module_id":"osc","key":"frequency","value":440.0,"intent":"author"}]}"#;
    let parsed: RpcCommand = serde_json::from_str(batch).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), batch);
}

#[test]
fn a_write_timed_to_a_beat_or_a_request_parses() {
    let at_bar = command(serde_json::json!({
        "command": "set_control", "module_id": "osc", "key": "frequency",
        "value": 330.0, "intent": "perform", "source": "agent",
        "when": { "kind": "next_bar", "clock": "clock" }, "ttl_ms": 2000
    }))
    .unwrap();
    let RpcCommand::SetControl { timing, .. } = &at_bar else {
        panic!("{at_bar:?}");
    };
    let next_bar = ControlWhen::NextBar {
        beats_per_bar: 4.0,
        clock: Some("clock".into()),
    };
    assert_eq!(timing.when.as_ref(), Some(&next_bar));
    assert_eq!(timing.ttl_ms, Some(2000));
    assert!(timing.is_set());

    let batch = command(serde_json::json!({
        "command": "set_controls", "writes": [
            { "module_id": "osc", "key": "frequency", "value": 220.0 },
            { "module_id": "env", "key": "gate", "value": true,
              "when": { "kind": "after_request", "request": "12", "offset_samples": 480 } }
        ]
    }))
    .unwrap();
    let RpcCommand::SetControls { writes } = &batch else {
        panic!("{batch:?}");
    };
    assert_eq!(writes[0].timing, ControlTiming::default());
    let after = ControlWhen::AfterRequest {
        request: "12".into(),
        offset_samples: 480,
    };
    assert_eq!(writes[1].timing.when.as_ref(), Some(&after));
    let round_trip: RpcCommand =
        serde_json::from_value(serde_json::to_value(&batch).unwrap()).unwrap();
    assert_eq!(round_trip, batch);
}

#[test]
fn an_offset_left_out_is_none_and_a_misspelt_time_is_refused() {
    let when: ControlWhen =
        serde_json::from_value(serde_json::json!({ "kind": "after_request", "request": "3" }))
            .unwrap();
    let at_once = ControlWhen::AfterRequest {
        request: "3".into(),
        offset_samples: 0,
    };
    assert_eq!(when, at_once);
    assert_eq!(
        serde_json::to_value(&when).unwrap(),
        serde_json::json!({ "kind": "after_request", "request": "3" })
    );
    let misspelt = command(serde_json::json!({
        "command": "set_control", "module_id": "osc", "key": "frequency", "value": 1.0,
        "when": { "kind": "after_request", "request": "3", "offset": 10 }
    }));
    assert!(misspelt.is_err(), "{misspelt:?}");
}

fn beat() -> LoggedBeat {
    LoggedBeat::Grid {
        clock: "clock".into(),
        every: 4.0,
        offset: 0.0,
        repeat: false,
    }
}

#[test]
fn a_reply_and_a_listing_say_where_a_timed_write_lands() {
    let landing = RequestLanding {
        sample: None,
        due: None,
        beat: Some(beat()),
    };
    let reply = AppliedControl {
        request: Some("9".into()),
        state: Some(ControlRequestState::Pending),
        requested: Some(ControlValue::Number(330.0)),
        landing: Some(landing.clone()),
        ..AppliedControl::read_back("osc", "frequency", None)
    };
    let json = serde_json::to_value(&reply).unwrap();
    assert_eq!(json["landing"]["beat"]["every"], 4.0, "{json}");
    assert_eq!(
        serde_json::from_value::<AppliedControl>(json).unwrap(),
        reply
    );
    // An untimed write's reply carries no landing.
    let untimed = AppliedControl::read_back("osc", "frequency", None);
    assert!(serde_json::to_value(&untimed)
        .unwrap()
        .get("landing")
        .is_none());

    let item = PendingRequest {
        request: "9".into(),
        module_id: "osc".into(),
        key: "frequency".into(),
        requested: ControlValue::Number(330.0),
        source: PendingSource::Agent,
        waiting: PendingWait::Beat,
        landing: Some(landing),
    };
    let json = serde_json::to_value(&item).unwrap();
    assert_eq!(
        serde_json::from_value::<PendingRequest>(json).unwrap(),
        item
    );
}

#[test]
fn a_settled_write_is_announced_with_its_state() {
    let expired = RpcEventPayload::ControlRequestSettled(ControlRequestSettled {
        request: "9".into(),
        module_id: "osc".into(),
        key: "frequency".into(),
        state: ControlRequestState::Expired,
        landing: RequestLanding::default(),
        reason: Some("its ttl passed before its time came".into()),
    });
    let json = serde_json::to_value(&expired).unwrap();
    assert_eq!(json["event"], "control_request_settled", "{json}");
    assert_eq!(json["state"], "expired", "{json}");
    assert_eq!(
        serde_json::from_value::<RpcEventPayload>(json).unwrap(),
        expired
    );
}
