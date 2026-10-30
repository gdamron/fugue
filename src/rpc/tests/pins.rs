use super::*;
use serde_json::json;

fn running(module: &str, key: &str) -> bool {
    matches!(
        (module, key),
        ("osc", "frequency") | ("osc", "waveform") | ("filter", "cutoff") | ("mixer", "master")
    )
}

fn at(module_id: &str, key: &str) -> ControlAddress {
    ControlAddress {
        module_id: module_id.into(),
        key: key.into(),
    }
}

fn pin(addresses: &[(&str, &str)], source: RequestSource) -> PinEdit {
    PinEdit {
        pin: addresses.iter().map(|(m, k)| at(m, k)).collect(),
        unpin: Vec::new(),
        source,
    }
}

fn addresses(pins: &[ControlPin]) -> Vec<ControlAddress> {
    pins.iter().map(ControlPin::address).collect()
}

#[test]
fn an_edit_pins_request_reads_from_the_wire() {
    let request: RpcRequest = serde_json::from_value(json!({
        "schema_version": RPC_SCHEMA_VERSION,
        "kind": "edit_pins",
        "pin": [{ "module_id": "osc", "key": "frequency" }],
        "source": "agent"
    }))
    .unwrap();
    assert_eq!(
        request.payload,
        RpcRequestPayload::EditPins(pin(&[("osc", "frequency")], RequestSource::Agent))
    );
    assert_eq!(request.payload.replay_policy(), ReplayPolicy::Resend);
    let get: RpcRequest = serde_json::from_value(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "get_pins"
    }))
    .unwrap();
    assert_eq!(get.payload, RpcRequestPayload::GetPins);
    assert_eq!(get.payload.replay_policy(), ReplayPolicy::Resend);
    // Empty lists stay off the wire.
    let wire = serde_json::to_value(RpcRequestPayload::EditPins(PinEdit::default())).unwrap();
    assert_eq!(wire, json!({ "kind": "edit_pins", "source": "user" }));
}

#[test]
fn the_reply_and_event_carry_the_set() {
    let list = PinList {
        pins: vec![ControlPin {
            module_id: "osc".into(),
            key: "frequency".into(),
            added_by: RequestSource::Agent,
            value: Some(crate::ControlValue::Number(220.0)),
        }],
        dropped: vec![at("bass", "cutoff")],
    };
    let reply = serde_json::to_value(RpcResponsePayload::Pins(list.clone())).unwrap();
    assert_eq!(
        reply,
        json!({
            "kind": "pins",
            "pins": [{ "module_id": "osc", "key": "frequency", "added_by": "agent", "value": 220.0 }],
            "dropped": [{ "module_id": "bass", "key": "cutoff" }]
        })
    );
    let event = serde_json::to_value(RpcEventPayload::PinsChanged(PinList::default())).unwrap();
    assert_eq!(event, json!({ "event": "pins_changed", "pins": [] }));
}

#[test]
fn pins_keep_their_order_and_who_pinned_them_without_duplicates() {
    let mut pins = Vec::new();
    pin(
        &[("osc", "frequency"), ("filter", "cutoff")],
        RequestSource::Agent,
    )
    .apply(&mut pins, running)
    .unwrap();
    pin(
        &[
            ("filter", "cutoff"),
            ("osc", "frequency"),
            ("mixer", "master"),
        ],
        RequestSource::User,
    )
    .apply(&mut pins, running)
    .unwrap();
    assert_eq!(
        addresses(&pins),
        vec![
            at("osc", "frequency"),
            at("filter", "cutoff"),
            at("mixer", "master")
        ]
    );
    let sources: Vec<_> = pins.iter().map(|p| p.added_by).collect();
    assert_eq!(
        sources,
        vec![
            RequestSource::Agent,
            RequestSource::Agent,
            RequestSource::User
        ]
    );
}

#[test]
fn unpinning_comes_first_and_ignores_what_is_not_pinned() {
    let mut pins = Vec::new();
    pin(
        &[("osc", "frequency"), ("filter", "cutoff")],
        RequestSource::User,
    )
    .apply(&mut pins, running)
    .unwrap();
    PinEdit {
        pin: vec![at("osc", "frequency")],
        unpin: vec![at("osc", "frequency"), at("gone", "level")],
        source: RequestSource::Agent,
    }
    .apply(&mut pins, running)
    .unwrap();
    // Unpinned and pinned again: now last, and the agent's.
    assert_eq!(
        addresses(&pins),
        vec![at("filter", "cutoff"), at("osc", "frequency")]
    );
    assert_eq!(pins[1].added_by, RequestSource::Agent);
}

#[test]
fn a_control_not_running_is_refused_and_nothing_changes() {
    let mut pins = Vec::new();
    pin(&[("osc", "frequency")], RequestSource::User)
        .apply(&mut pins, running)
        .unwrap();
    let error = PinEdit {
        pin: vec![
            at("filter", "cutoff"),
            at("osc", "detune"),
            at("gone", "level"),
        ],
        unpin: vec![at("osc", "frequency")],
        source: RequestSource::User,
    }
    .apply(&mut pins, running)
    .unwrap_err();
    assert_eq!(error.code, RpcErrorCode::UnknownModule);
    assert!(
        error.message.contains("osc.detune, gone.level"),
        "{}",
        error.message
    );
    assert_eq!(addresses(&pins), vec![at("osc", "frequency")]);
}

#[test]
fn the_set_is_capped_and_a_refused_edit_changes_nothing() {
    let everything = |_: &str, _: &str| true;
    let mut pins = Vec::new();
    let full: Vec<ControlAddress> = (0..MAX_PINNED_CONTROLS)
        .map(|i| at("mixer", &format!("level.{i}")))
        .collect();
    PinEdit {
        pin: full.clone(),
        ..PinEdit::default()
    }
    .apply(&mut pins, everything)
    .unwrap();
    let error = pin(&[("osc", "frequency")], RequestSource::Agent)
        .apply(&mut pins, everything)
        .unwrap_err();
    assert_eq!(error.code, RpcErrorCode::InvalidRequest);
    assert!(
        error.message.contains("unpin some first"),
        "{}",
        error.message
    );
    assert_eq!(addresses(&pins), full);
    // Making room in the same edit is fine.
    PinEdit {
        pin: vec![at("osc", "frequency")],
        unpin: vec![full[0].clone()],
        source: RequestSource::Agent,
    }
    .apply(&mut pins, everything)
    .unwrap();
    assert_eq!(pins.len(), MAX_PINNED_CONTROLS);
}

#[test]
fn a_control_named_twice_in_one_edit_is_pinned_once() {
    let mut pins = Vec::new();
    pin(
        &[("osc", "frequency"), ("osc", "frequency")],
        RequestSource::Agent,
    )
    .apply(&mut pins, running)
    .unwrap();
    assert_eq!(addresses(&pins), vec![at("osc", "frequency")]);
}

#[test]
fn an_edit_naming_more_controls_than_the_set_holds_is_refused() {
    let mut pins = Vec::new();
    let many: Vec<ControlAddress> = (0..=MAX_PINNED_CONTROLS)
        .map(|i| at(&format!("m{i}"), "key"))
        .collect();
    for edit in [
        PinEdit {
            pin: many.clone(),
            ..PinEdit::default()
        },
        PinEdit {
            unpin: many.clone(),
            ..PinEdit::default()
        },
    ] {
        let error = edit.apply(&mut pins, |_, _| true).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::InvalidRequest);
    }
    assert!(pins.is_empty());
}

#[test]
fn a_name_past_the_limit_is_refused() {
    let mut pins = Vec::new();
    let long = "x".repeat(MAX_PIN_NAME_BYTES + 1);
    let error = PinEdit {
        unpin: vec![at(&long, "key")],
        ..PinEdit::default()
    }
    .apply(&mut pins, |_, _| true)
    .unwrap_err();
    assert_eq!(error.code, RpcErrorCode::InvalidRequest);
}

#[test]
fn retaining_drops_pins_whose_control_is_gone() {
    let mut pins = Vec::new();
    pin(
        &[
            ("osc", "frequency"),
            ("filter", "cutoff"),
            ("osc", "waveform"),
        ],
        RequestSource::Agent,
    )
    .apply(&mut pins, running)
    .unwrap();
    let dropped = retain_pins(&mut pins, |module, key| {
        module == "osc" && key != "waveform"
    });
    assert_eq!(dropped, vec![at("filter", "cutoff"), at("osc", "waveform")]);
    assert_eq!(addresses(&pins), vec![at("osc", "frequency")]);
}
