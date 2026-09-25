use super::*;
use crate::ControlValue;

#[test]
fn poll_events_request_and_page_round_trip() {
    use crate::{EventPage, RpcEventPayload, SeqEvent};

    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
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
        dropped: false,
    };
    let response = RpcResponse::ok(None, RpcResponsePayload::Events(page.clone()));
    let decoded: RpcResponse =
        serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Events(page));
}

#[test]
fn get_meters_request_and_reply_round_trip() {
    use crate::MeterReading;

    let request = RpcRequest {
        schema_version: RPC_SCHEMA_VERSION,
        expected_revision: None,
        request_id: Some("meters".to_string()),
        payload: RpcRequestPayload::GetMeters,
    };
    let decoded: RpcRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(decoded.payload, RpcRequestPayload::GetMeters);

    let bare: RpcRequest =
        serde_json::from_str(r#"{"schema_version":1,"kind":"get_meters"}"#).unwrap();
    assert_eq!(bare.payload, RpcRequestPayload::GetMeters);

    let meters = vec![MeterReading {
        sink_id: "master".to_string(),
        left_peak: 0.5,
        right_peak: 0.25,
    }];
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
