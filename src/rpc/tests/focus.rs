use super::*;
use serde_json::json;

fn running(id: &str) -> bool {
    matches!(id, "melody" | "bass" | "mixer")
}

fn intent(value: serde_json::Value) -> FocusIntent {
    serde_json::from_value(value).unwrap()
}

fn select(focus: &mut Option<Focus>, value: serde_json::Value) -> Result<(), RpcError> {
    intent(value).apply(focus, 1_000, running)
}

#[test]
fn a_views_select_detail_is_a_set_focus_request_as_is() {
    // The piano roll's `fyug-select` detail, note ids and all, with the
    // request envelope around it.
    let request: RpcRequest = serde_json::from_value(json!({
        "schema_version": RPC_SCHEMA_VERSION,
        "kind": "set_focus",
        "view": "piano-roll",
        "module_id": "melody",
        "selection": {
            "kind": "notes",
            "unit": "beats",
            "notes": [{ "id": "n7", "voice": "0", "pitch": 64, "start": 8.0, "end": 9.0 }]
        },
        "beats_per_bar": 4.0
    }))
    .unwrap();
    let RpcRequestPayload::SetFocus(intent) = request.payload else {
        panic!("not a set_focus request");
    };
    assert_eq!(intent.module_id.as_deref(), Some("melody"));
    let mut focus = None;
    intent.apply(&mut focus, 42, running).unwrap();
    let focus = focus.unwrap();
    assert_eq!(focus.bars, Some(FocusBars { first: 3, last: 3 }));
    assert_eq!(focus.selected_at_ms, 42);
    assert_eq!(
        serde_json::to_value(&focus).unwrap()["selection"]["notes"][0],
        json!({ "voice": "0", "pitch": 64, "start": 8.0, "end": 9.0 })
    );

    let get: RpcRequest = serde_json::from_value(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "get_focus"
    }))
    .unwrap();
    assert_eq!(get.payload, RpcRequestPayload::GetFocus);
}

#[test]
fn a_beat_range_reads_as_the_bars_it_covers() {
    let mut focus = None;
    let range = |start: f64, end: f64| {
        json!({
            "view": "piano-roll", "module_id": "melody", "beats_per_bar": 4.0,
            "selection": { "kind": "time-range", "unit": "beats", "start": start, "end": end }
        })
    };
    // Bars 3-4: beats 8 up to 16, an end on the bar line closing bar 4.
    select(&mut focus, range(8.0, 16.0)).unwrap();
    assert_eq!(
        focus.as_ref().unwrap().bars,
        Some(FocusBars { first: 3, last: 4 })
    );
    select(&mut focus, range(9.5, 10.0)).unwrap();
    assert_eq!(
        focus.as_ref().unwrap().bars,
        Some(FocusBars { first: 3, last: 3 })
    );
    // A moment on a bar line is in the bar it starts.
    select(&mut focus, range(4.0, 4.0)).unwrap();
    assert_eq!(
        focus.as_ref().unwrap().bars,
        Some(FocusBars { first: 2, last: 2 })
    );
}

#[test]
fn seconds_and_a_missing_bar_length_name_no_bars() {
    let mut focus = None;
    select(
        &mut focus,
        json!({ "view": "waveform", "module_id": "bass", "beats_per_bar": 4.0,
                "selection": { "kind": "time-range", "unit": "seconds", "start": 1.0, "end": 2.5 } }),
    )
    .unwrap();
    assert_eq!(focus.as_ref().unwrap().bars, None);
    select(
        &mut focus,
        json!({ "view": "piano-roll", "module_id": "melody",
                "selection": { "kind": "time-range", "unit": "beats", "start": 0.0, "end": 4.0 } }),
    )
    .unwrap();
    assert_eq!(focus.as_ref().unwrap().bars, None);
}

#[test]
fn a_clear_drops_only_its_own_views_selection() {
    let mut focus = None;
    select(
        &mut focus,
        json!({ "view": "graph", "module_id": null,
                "selection": { "kind": "graph", "nodes": ["bass"], "connections": [] } }),
    )
    .unwrap();
    // The piano roll letting go does not erase the graph's newer selection.
    select(
        &mut focus,
        json!({ "view": "piano-roll", "module_id": "melody" }),
    )
    .unwrap();
    assert_eq!(focus.as_ref().unwrap().view, "graph");
    select(&mut focus, json!({ "view": "graph", "module_id": null })).unwrap();
    assert_eq!(focus, None);
}

#[test]
fn a_selection_of_a_module_not_running_is_refused_and_changes_nothing() {
    let mut focus = None;
    select(
        &mut focus,
        json!({ "view": "piano-roll", "module_id": "melody",
                "selection": { "kind": "voice", "voice": "0" } }),
    )
    .unwrap();
    let before = focus.clone();
    let error = select(
        &mut focus,
        json!({ "view": "graph", "selection": { "kind": "graph", "nodes": ["ghost"] } }),
    )
    .unwrap_err();
    assert_eq!(error.code, RpcErrorCode::UnknownModule);
    assert_eq!(focus, before);
}

#[test]
fn malformed_selections_are_refused() {
    let mut focus = None;
    let refused = |focus: &mut Option<Focus>, value| select(focus, value).unwrap_err().code;
    let backwards = json!({ "view": "piano-roll", "module_id": "melody",
        "selection": { "kind": "time-range", "unit": "beats", "start": 4.0, "end": 2.0 } });
    assert_eq!(refused(&mut focus, backwards), RpcErrorCode::InvalidRequest);
    let empty_graph = json!({ "view": "graph", "selection": { "kind": "graph" } });
    assert_eq!(
        refused(&mut focus, empty_graph),
        RpcErrorCode::InvalidRequest
    );
    let bad_bar = json!({ "view": "piano-roll", "module_id": "melody", "beats_per_bar": 0.0,
        "selection": { "kind": "voice", "voice": "0" } });
    assert_eq!(refused(&mut focus, bad_bar), RpcErrorCode::InvalidRequest);
    let many: Vec<_> = (0..=MAX_FOCUS_ITEMS)
        .map(|i| json!({ "voice": "0", "pitch": 60, "start": i as f64 }))
        .collect();
    let too_many = json!({ "view": "piano-roll", "module_id": "melody",
        "selection": { "kind": "notes", "unit": "beats", "notes": many } });
    assert_eq!(refused(&mut focus, too_many), RpcErrorCode::InvalidRequest);
    let unnamed = json!({ "view": "", "selection": { "kind": "voice", "voice": "0" } });
    assert_eq!(refused(&mut focus, unnamed), RpcErrorCode::InvalidRequest);
    assert_eq!(focus, None);
}

#[test]
fn focus_keeps_to_the_modules_still_running() {
    let mut focus = None;
    select(
        &mut focus,
        json!({ "view": "graph", "selection": { "kind": "graph",
            "nodes": ["melody", "bass"],
            "connections": [{ "from": "melody", "from_port": "audio", "to": "mixer", "to_port": "in1" }] } }),
    )
    .unwrap();
    let mut focus = focus.unwrap();
    // The melody is removed: the bass stays selected.
    assert!(focus.retain_modules(|id| id != "melody"));
    assert_eq!(
        focus.selection,
        FocusTarget::Graph {
            nodes: vec!["bass".to_string()],
            connections: vec![]
        }
    );
    assert!(!focus.retain_modules(|_| false));

    let mut roll = None;
    select(
        &mut roll,
        json!({ "view": "piano-roll", "module_id": "melody",
                "selection": { "kind": "voice", "voice": "0" } }),
    )
    .unwrap();
    let mut roll = roll.unwrap();
    assert!(roll.retain_modules(running));
    assert!(!roll.retain_modules(|id| id != "melody"));
}

#[test]
fn the_focus_reply_round_trips() {
    let reply = RpcResponse::ok(None, RpcResponsePayload::Focus { focus: None });
    let text = serde_json::to_string(&reply).unwrap();
    assert!(text.contains("\"kind\":\"focus\""), "{text}");
    let decoded: RpcResponse = serde_json::from_str(&text).unwrap();
    assert_eq!(decoded.payload, RpcResponsePayload::Focus { focus: None });
}
