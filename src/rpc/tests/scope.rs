use super::*;
use crate::{ChannelBins, OutputBins};
use serde_json::json;

fn request(value: serde_json::Value) -> RpcRequestPayload {
    serde_json::from_value::<RpcRequest>(value).unwrap().payload
}

fn bins(min: Vec<f32>, max: Vec<f32>) -> OutputBins {
    OutputBins {
        module_id: "voice".to_string(),
        ports: vec!["audio".to_string()],
        sample_rate: 48_000,
        samples_per_bin: 96,
        origin_frame: 9_600,
        channels: vec![ChannelBins { min, max }],
        lost: 192,
        open: true,
    }
}

#[test]
fn open_scope_names_a_module_and_polls_by_default() {
    let bare = request(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "open_scope", "module_id": "voice"
    }));
    assert_eq!(
        bare,
        RpcRequestPayload::OpenScope(OpenScope {
            module_id: "voice".to_string(),
            ports: Vec::new(),
            samples_per_bin: None,
            delivery: ScopeDelivery::Poll,
        })
    );

    let streamed = request(json!({
        "schema_version": RPC_SCHEMA_VERSION,
        "kind": "open_scope",
        "module_id": "lfo",
        "ports": ["out"],
        "samples_per_bin": 128,
        "delivery": "stream"
    }));
    let RpcRequestPayload::OpenScope(open) = streamed else {
        panic!("not open_scope");
    };
    assert_eq!(open.ports, ["out"]);
    assert_eq!(open.samples_per_bin, Some(128));
    assert_eq!(open.delivery, ScopeDelivery::Stream);

    let read = request(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "read_scope", "scope_id": "s1"
    }));
    assert_eq!(
        read,
        RpcRequestPayload::ReadScope {
            scope_id: "s1".to_string()
        }
    );
    let close = request(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "close_scope", "scope_id": "s1"
    }));
    assert_eq!(
        close,
        RpcRequestPayload::CloseScope {
            scope_id: "s1".to_string()
        }
    );
}

#[test]
fn a_gap_is_null_in_min_and_max_together() {
    // A bin of nothing but NaN reads NaN from the tap.
    let frame = ScopeFrame::from_bins(
        "s1",
        bins(
            vec![-0.5, f32::NAN, -0.25, f32::NAN],
            vec![0.5, f32::NAN, 0.25, 0.1],
        ),
    );
    assert_eq!(frame.len(), 4);
    let wire = serde_json::to_value(&frame).unwrap();
    assert_eq!(
        wire,
        json!({
            "scope_id": "s1",
            "module_id": "voice",
            "sample_rate": 48_000,
            "samples_per_bin": 96,
            "origin_frame": 9_600,
            "channels": [{
                "label": "audio",
                "min": [-0.5, null, -0.25, null],
                "max": [0.5, null, 0.25, null]
            }],
            "lost": 192,
            "open": true
        })
    );
    let back: ScopeFrame = serde_json::from_value(wire).unwrap();
    assert_eq!(back, frame);
}

#[test]
fn an_infinite_value_reads_as_the_largest_finite_one() {
    let frame = ScopeFrame::from_bins(
        "s1",
        bins(vec![f32::NEG_INFINITY, -0.5], vec![0.1, f32::INFINITY]),
    );
    assert_eq!(frame.channels[0].min, [Some(f32::MIN), Some(-0.5)]);
    assert_eq!(frame.channels[0].max, [Some(0.1), Some(f32::MAX)]);
    // It serializes: JSON carries every finite f32.
    serde_json::to_string(&frame).unwrap();
}

#[test]
fn values_past_full_scale_pass_through_unclipped() {
    let frame = ScopeFrame::from_bins("s1", bins(vec![-1.5], vec![2.0]));
    assert_eq!(frame.channels[0].min, [Some(-1.5)]);
    assert_eq!(frame.channels[0].max, [Some(2.0)]);
}

#[test]
fn replies_and_events_are_tagged_for_clients() {
    let info = ScopeInfo {
        scope_id: "s1".to_string(),
        module_id: "voice".to_string(),
        ports: vec!["audio_left".to_string(), "audio_right".to_string()],
        sample_rate: 48_000,
        samples_per_bin: 96,
        delivery: ScopeDelivery::Poll,
        encoding: ScopeEncoding::F32Json,
        frame_interval_ms: 33,
        idle_close_ms: Some(5_000),
    };
    let opened = RpcResponse::ok(None, RpcResponsePayload::ScopeOpened(info.clone()));
    let wire = serde_json::to_value(&opened).unwrap();
    assert_eq!(wire["kind"], "scope_opened");
    assert_eq!(wire["delivery"], "poll");
    assert_eq!(wire["encoding"], "f32_json");
    assert_eq!(wire["idle_close_ms"], 5_000);
    let back: RpcResponse = serde_json::from_value(wire).unwrap();
    assert_eq!(back.payload, RpcResponsePayload::ScopeOpened(info));

    let frame = ScopeFrame::from_bins("s1", bins(vec![0.0], vec![0.0]));
    let reply = RpcResponse::ok(None, RpcResponsePayload::ScopeFrame(frame.clone()));
    assert_eq!(serde_json::to_value(&reply).unwrap()["kind"], "scope_frame");
    let event = RpcEvent::new(RpcEventPayload::ScopeFrame(frame));
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(wire["event"], "scope_frame");
    assert_eq!(wire["channels"][0]["label"], "audio");
    assert_eq!(
        serde_json::to_value(RpcSubscriptionTopic::Scopes).unwrap(),
        "scopes"
    );
}

#[test]
fn an_unknown_encoding_reads_as_unsupported() {
    let encoding: ScopeEncoding = serde_json::from_value(json!("u8_base64")).unwrap();
    assert_eq!(encoding, ScopeEncoding::Unsupported);
}

#[test]
fn scope_requests_resend_after_a_lost_reply() {
    // A scope opened by a lost reply closes with the connection that lost it.
    for payload in [
        request(json!({ "schema_version": 1, "kind": "open_scope", "module_id": "v" })),
        request(json!({ "schema_version": 1, "kind": "read_scope", "scope_id": "s" })),
        request(json!({ "schema_version": 1, "kind": "close_scope", "scope_id": "s" })),
    ] {
        assert_eq!(payload.replay_policy(), ReplayPolicy::Resend);
    }
}
