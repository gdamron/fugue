use super::*;
use crate::ControlValue;

#[test]
fn poll_events_request_and_page_round_trip() {
    use crate::{EventPage, RpcEventPayload, SeqEvent};

    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        mutation: None,
        reply: MutationReply::Snapshot,
        request_id: Some("poll".to_string()),
        payload: RpcRequestPayload::PollEvents { after: Some(7) },
    };
    let decoded: RpcRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(
        decoded.payload,
        RpcRequestPayload::PollEvents { after: Some(7) }
    );

    // `after` is optional on the wire.
    let bare: RpcRequest =
        serde_json::from_str(r#"{"schema_version":1,"kind":"poll_events"}"#).unwrap();
    assert_eq!(bare.payload, RpcRequestPayload::PollEvents { after: None });

    let page = EventPage {
        events: vec![SeqEvent {
            seq: 8,
            payload: RpcEventPayload::ControlChanged {
                module_id: "mixer".to_string(),
                key: "master".to_string(),
                value: ControlValue::Number(0.7),
            },
        }],
        latest_seq: 8,
        resync_required: false,
    };
    let wire = serde_json::to_value(&page).unwrap();
    assert_eq!(wire["resync_required"], false);
    assert!(wire.get("dropped").is_none(), "`dropped` left the wire");
    let response = RpcResponse::ok(None, RpcResponsePayload::Events(page.clone()));
    let decoded: RpcResponse =
        serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Events(page));
}

#[test]
fn get_meters_request_and_reply_round_trip() {
    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        mutation: None,
        reply: MutationReply::Snapshot,
        request_id: Some("meters".to_string()),
        payload: RpcRequestPayload::GetMeters,
    };
    let decoded: RpcRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcRequestPayload::GetMeters);

    let bare: RpcRequest =
        serde_json::from_str(r#"{"schema_version":1,"kind":"get_meters"}"#).unwrap();
    assert_eq!(bare.payload, RpcRequestPayload::GetMeters);

    let meters = vec![measured_reading()];
    let response = RpcResponse::ok(
        None,
        RpcResponsePayload::Meters {
            meters: meters.clone(),
        },
    );
    let decoded: RpcResponse =
        serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Meters { meters });
}

/// A reading with every field, two seconds into a 48 kHz audio clock.
fn measured_reading() -> crate::MeterReading {
    crate::MeterReading::new(
        "master",
        &crate::MasterLevels {
            left_peak: 0.5,
            right_peak: 0.25,
            left_rms: Some(0.125),
            right_rms: None,
            end_frame: 96_000,
            frames: 4_800,
            sample_rate: 48_000,
        },
    )
}

#[test]
fn meter_level_carries_rms_and_the_audio_clock_flat_on_the_wire() {
    use crate::{RpcEvent, RpcEventPayload};

    let event = RpcEvent::new(RpcEventPayload::MeterLevel(measured_reading()));
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({
            "schema_version": RPC_SCHEMA_VERSION,
            "event": "meter_level",
            "sink_id": "master",
            "left_peak": 0.5,
            "right_peak": 0.25,
            // An RMS that was not measured is left out, not sent as null.
            "left_rms": 0.125,
            "audio_clock_frame": 96_000,
            "audio_clock_ms": 2_000.0,
            "sample_rate": 48_000,
        })
    );
    let decoded: RpcEvent = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded, event);
}

/// Readings from a daemon that predates RMS and the audio clock still
/// decode, with those fields absent.
#[test]
fn a_peak_only_meter_level_from_an_older_daemon_still_decodes() {
    use crate::{RpcEvent, RpcEventPayload};

    let old = r#"{"schema_version":1,"event":"meter_level","sink_id":"master","left_peak":0.5,"right_peak":0.25}"#;
    let decoded: RpcEvent = serde_json::from_str(old).unwrap();
    assert_eq!(
        decoded.payload,
        RpcEventPayload::MeterLevel(crate::MeterReading {
            sink_id: "master".to_string(),
            left_peak: 0.5,
            right_peak: 0.25,
            ..Default::default()
        })
    );
}
