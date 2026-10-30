use super::*;
use crate::ControlValue;
use serde_json::json;

fn long(len: usize) -> String {
    "é".repeat(len / 2 + 1)
}

/// A summary with every list full and every name far past its limit.
fn worst_case() -> StatusSummary {
    StatusSummary {
        version: STATUS_SUMMARY_VERSION,
        seq: u64::MAX,
        published_at_ms: u64::MAX,
        revision: Some(RuntimeRevision {
            session_id: long(64),
            revision: u64::MAX,
        }),
        loaded: Some(LoadedSummary {
            title: Some(long(10_000)),
            file: Some(long(10_000)),
            module_count: usize::MAX,
            connection_count: usize::MAX,
            modules: (0..1_000)
                .map(|_| SummaryModule {
                    id: long(1_000),
                    module_type: long(1_000),
                })
                .collect(),
        }),
        transport: TransportSummary {
            playing: true,
            clocks: (0..100)
                .map(|_| ClockSummary {
                    id: long(1_000),
                    bpm: Some(f32::MAX),
                    beats: Some(f64::MAX),
                })
                .collect(),
        },
        pinned: Some(
            (0..1_000)
                .map(|_| PinnedControl {
                    module_id: long(1_000),
                    key: long(1_000),
                    value: ControlValue::String(long(10_000)),
                })
                .collect(),
        ),
        levels: Some(OutputLevels {
            left_peak: f32::MAX,
            right_peak: f32::MAX,
            left_rms: Some(f32::MAX),
            right_rms: Some(f32::MAX),
        }),
        session: Some(SessionDurability {
            state: SessionSaveState::Failing,
            saved_revision: Some(RuntimeRevision {
                session_id: long(64),
                revision: u64::MAX,
            }),
            behind_ms: Some(u64::MAX),
            error: Some(long(100_000)),
        }),
        focus: Some(FocusSummary {
            view: long(1_000),
            module_id: Some(long(1_000)),
            kind: long(1_000),
            bars: Some(FocusBars {
                first: u64::MAX,
                last: u64::MAX,
            }),
            selected_at_ms: u64::MAX,
        }),
    }
}

#[test]
fn a_full_summary_is_bounded_without_losing_its_counts() {
    let summary = worst_case().bounded();
    assert!(
        summary.serialized_len() <= MAX_STATUS_SUMMARY_BYTES,
        "{} bytes",
        summary.serialized_len()
    );
    let loaded = summary.loaded.as_ref().unwrap();
    assert_eq!(loaded.module_count, usize::MAX);
    assert!(loaded.title.as_ref().unwrap().len() <= MAX_SUMMARY_TEXT_BYTES);
    assert!(loaded.modules.len() <= MAX_SUMMARY_MODULES);
    assert!(summary.transport.clocks.len() <= MAX_SUMMARY_CLOCKS);
    assert!(summary.pinned.as_ref().unwrap().len() <= MAX_SUMMARY_PINNED);
    // Cut at a character boundary: still valid text.
    assert!(loaded
        .modules
        .iter()
        .all(|m| m.id.chars().all(|c| c == 'é')));
}

#[test]
fn names_that_escape_long_give_up_modules_not_the_limit() {
    let mut summary = worst_case();
    // Control characters escape six bytes for one.
    for module in &mut summary.loaded.as_mut().unwrap().modules {
        module.id = "\u{1}".repeat(64);
    }
    let summary = summary.bounded();
    assert!(summary.serialized_len() <= MAX_STATUS_SUMMARY_BYTES);
    assert!(summary.loaded.unwrap().modules.len() < MAX_SUMMARY_MODULES);
}

#[test]
fn the_wire_shape_is_versioned_and_says_what_is_not_available() {
    let summary = StatusSummary {
        version: STATUS_SUMMARY_VERSION,
        seq: 3,
        published_at_ms: 1_000,
        revision: None,
        loaded: None,
        transport: TransportSummary::default(),
        pinned: None,
        levels: None,
        session: None,
        focus: None,
    };
    let response = RpcResponse::ok(
        Some("r1".to_string()),
        RpcResponsePayload::Summary {
            summary: summary.clone(),
        },
    );
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(
        value["summary"],
        json!({
            "version": 1, "seq": 3, "published_at_ms": 1_000,
            "loaded": null, "transport": { "playing": false, "clocks": [] },
            "pinned": null, "levels": null
        })
    );
    let back: RpcResponse = serde_json::from_value(value).unwrap();
    assert_eq!(back.payload, RpcResponsePayload::Summary { summary });

    let request: RpcRequest = serde_json::from_value(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "get_summary"
    }))
    .unwrap();
    assert_eq!(request.payload, RpcRequestPayload::GetSummary);
    assert_eq!(request.payload.replay_policy(), ReplayPolicy::Resend);
}

#[test]
fn a_reader_ignores_fields_a_later_version_adds() {
    let summary: StatusSummary = serde_json::from_value(json!({
        "version": 1, "seq": 1, "published_at_ms": 0,
        "loaded": null, "transport": { "playing": false, "clocks": [], "loop": true },
        "pinned": [], "levels": null, "tempo_map": []
    }))
    .unwrap();
    assert_eq!(summary.pinned, Some(Vec::new()));
}
