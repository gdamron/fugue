use super::*;
use crate::Invention;
use serde_json::json;

fn snapshot(value: Value) -> AuthoredSnapshot {
    let mut document: Invention = serde_json::from_value(value).unwrap();
    document.source_path = Some("/music/layers/invention.json".into());
    AuthoredSnapshot::new(
        document,
        RuntimeRevision {
            session_id: "session".into(),
            revision: 7,
        },
    )
    .unwrap()
}

fn query(selection: InspectionSelection) -> InspectionQuery {
    InspectionQuery {
        selection,
        limit: 100,
        cursor: None,
    }
}

fn layered() -> AuthoredSnapshot {
    let mut modules = vec![
        json!({"id":"clock","type":"clock"}),
        json!({"id":"mix","type":"mixer","config":{"channels":32}}),
    ];
    let mut connections = Vec::new();
    for i in 0..32 {
        modules.push(json!({"id":format!("voice_{i}"),"type":"voice","config":{"attack":0.01}}));
        modules.push(json!({"id":format!("sequence_{i}"),"type":"cell_sequencer","config":{"sequences":[(0..128).map(|n| json!({"note":n%12,"amplitude":0.75,"gate_length":0.8})).collect::<Vec<_>>()],"base_note":{"$asset":"harmony","path":"/base_note"}}}));
        connections.extend([
            json!({"from":"clock","to":format!("sequence_{i}"),"from_port":"gate_x4","to_port":"gate"}),
            json!({"from":format!("sequence_{i}"),"to":format!("voice_{i}"),"from_port":"frequency","to_port":"frequency"}),
            json!({"from":format!("sequence_{i}"),"to":format!("voice_{i}"),"from_port":"gate","to_port":"gate"}),
            json!({"from":format!("voice_{i}"),"to":"mix","from_port":"audio","to_port":format!("in{}",i+1)}),
        ]);
    }
    snapshot(
        json!({"title":"Layered arrangement", "modules":modules,"connections":connections,
        "assets":{"harmony":{"path":"./harmony.json"}},
        "developments":[{"name":"voice","definition":{"modules":[{"id":"osc","type":"oscillator"},{"id":"env","type":"adsr"},{"id":"vca","type":"vca"}],"connections":[{"from":"osc","from_port":"audio","to":"vca","to_port":"audio"},{"from":"env","from_port":"envelope","to":"vca","to_port":"cv"}],"inputs":[{"name":"frequency","to":"osc","to_port":"frequency"},{"name":"gate","to":"env","to_port":"gate"}],"outputs":[{"name":"audio","from":"vca","from_port":"audio"}],"controls":[{"key":"attack","module":"env","control":"attack"}]}}]}),
    )
}

#[test]
fn layered_arrangement_context_and_call_count() {
    let snapshot = layered();
    // The workload is a buildable musical arrangement, not merely JSON padding.
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("harmony.json"), r#"{"base_note":48}"#).unwrap();
    let mut buildable = snapshot.document.clone();
    buildable.source_path = Some(directory.path().join("invention.json"));
    crate::InventionBuilder::new(48_000)
        .build(buildable)
        .unwrap();
    let before = snapshot.clone();
    let queries = [
        InspectionSelection::Overview { scope: "".into() },
        InspectionSelection::Module {
            scope: "".into(),
            id: "voice_17".into(),
        },
        InspectionSelection::Development {
            scope: "".into(),
            name: "voice".into(),
        },
        InspectionSelection::Module {
            scope: "".into(),
            id: "sequence_17".into(),
        },
    ];
    let queries = queries.into_iter().chain([
        InspectionSelection::Value {
            pointer: "/modules/37/config".into(),
        },
        InspectionSelection::Value {
            pointer: "/modules/37/config/sequences/0".into(),
        },
        InspectionSelection::Module {
            scope: "/developments/0/definition".into(),
            id: "env".into(),
        },
    ]);
    let mut bytes = 0;
    let mut calls = 0;
    for selection in queries {
        let mut q = query(selection);
        loop {
            let page = snapshot.inspect(&q).unwrap();
            bytes += serde_json::to_vec(&page).unwrap().len();
            calls += 1;
            assert_eq!(page.revision, snapshot.revision);
            assert_eq!(page.source_path, snapshot.source_path);
            if let Some(cursor) = page.next_cursor {
                q.cursor = Some(cursor);
            } else {
                break;
            }
        }
    }
    let full = snapshot
        .to_json(super::super::SnapshotDelivery::File)
        .unwrap()
        .len();
    let compact_full = serde_json::to_vec(&snapshot).unwrap().len();
    eprintln!("layered arrangement: full pretty={full} bytes, compact={compact_full} bytes / 1 retrieval; focused={bytes} bytes / {calls} calls; byte/4 token estimates: full={}, focused={}", full.div_ceil(4), bytes.div_ceil(4));
    assert!(bytes * 3 < compact_full, "{bytes} vs {compact_full}");
    assert_eq!(snapshot, before);
    let voice = snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "".into(),
            id: "voice_17".into(),
        }))
        .unwrap();
    assert!(voice
        .entries
        .iter()
        .any(|e| e.value.as_ref().is_some_and(|v| v["from"] == "sequence_17")));
    assert!(voice.entries.iter().any(|e| e.pointer == "/developments/0"));
    let sequence = snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "".into(),
            id: "sequence_17".into(),
        }))
        .unwrap();
    assert!(sequence
        .entries
        .iter()
        .any(|e| e.pointer == "/assets/harmony"));
}

#[test]
fn pages_reassemble_exact_values_and_reject_changed_revision_or_selection() {
    let mut snapshot = layered();
    let mut q = query(InspectionSelection::Value {
        pointer: "/connections".into(),
    });
    q.limit = 7;
    let mut collected = Vec::new();
    let first = snapshot.inspect(&q).unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    loop {
        let page = snapshot.inspect(&q).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_INSPECTION_BYTES);
        collected.extend(page.entries.into_iter().map(|e| e.value.unwrap()));
        if let Some(cursor) = page.next_cursor {
            q.cursor = Some(cursor);
        } else {
            break;
        }
    }
    assert_eq!(
        Value::Array(collected),
        serde_json::to_value(&snapshot.document.connections).unwrap()
    );
    q.cursor = Some(cursor);
    q.selection = InspectionSelection::Overview { scope: "".into() };
    assert_eq!(
        snapshot.inspect(&q).unwrap_err().code,
        RpcErrorCode::InvalidRequest
    );
    q.selection = q.cursor.as_ref().unwrap().selection.clone();
    snapshot.revision.revision += 1;
    assert_eq!(
        snapshot.inspect(&q).unwrap_err().conflict.unwrap().reason,
        ConflictReason::StaleRevision
    );
    snapshot.revision.session_id = "replacement".into();
    assert_eq!(
        snapshot.inspect(&q).unwrap_err().conflict.unwrap().reason,
        ConflictReason::SessionReplaced
    );
}

#[test]
fn oversized_config_drills_down_without_silent_truncation() {
    let snapshot = layered();
    let page = snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "".into(),
            id: "sequence_17".into(),
        }))
        .unwrap();
    let omitted = page
        .entries
        .iter()
        .find(|e| e.coverage == InspectionCoverage::Omitted)
        .unwrap();
    let config = snapshot
        .inspect(&query(InspectionSelection::Value {
            pointer: format!("{}/config", omitted.pointer),
        }))
        .unwrap();
    assert!(config
        .entries
        .iter()
        .any(|e| e.coverage == InspectionCoverage::Omitted && e.pointer.ends_with("/sequences")));
    let cells = snapshot
        .inspect(&query(InspectionSelection::Value {
            pointer: format!("{}/config/sequences/0", omitted.pointer),
        }))
        .unwrap();
    assert!(cells
        .entries
        .iter()
        .all(|e| e.coverage == InspectionCoverage::Complete));
    assert!(cells.next_cursor.is_some());
}

#[test]
fn nested_authored_scopes_expose_aliases_and_keep_external_references() {
    let mut snapshot = layered();
    snapshot
        .document
        .developments
        .push(serde_json::from_value(json!({"name":"external","path":"./voice.json"})).unwrap());
    let page = snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "/developments/0/definition".into(),
            id: "osc".into(),
        }))
        .unwrap();
    for pointer in [
        "/developments/0/definition/inputs/0",
        "/developments/0/definition/connections/0",
    ] {
        assert!(page
            .entries
            .iter()
            .any(|e| e.pointer == pointer && e.coverage == InspectionCoverage::Complete));
    }
    let page = snapshot
        .inspect(&query(InspectionSelection::Development {
            scope: "".into(),
            name: "external".into(),
        }))
        .unwrap();
    assert_eq!(
        page.entries[0].value.as_ref().unwrap()["path"],
        "./voice.json"
    );
    assert!(snapshot
        .inspect(&query(InspectionSelection::Overview {
            scope: "/developments/1/definition".into()
        }))
        .is_err());
}

#[test]
fn invalid_queries_and_escaped_asset_pointers() {
    let snapshot = snapshot(
        json!({"modules":[{"id":"voice","type":"oscillator","config":{"frequency":{"$asset":"a~/b"}}}],"connections":[],"assets":{"a~/b":{"path":"notes.json"}}}),
    );
    let page = snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "".into(),
            id: "voice".into(),
        }))
        .unwrap();
    let pointer = "/assets/a~0~1b";
    assert!(page.entries.iter().any(|e| e.pointer == pointer));
    assert!(snapshot
        .inspect(&query(InspectionSelection::Value {
            pointer: pointer.into()
        }))
        .is_ok());
    let mut q = query(InspectionSelection::Overview { scope: "".into() });
    for limit in [0, 101, usize::MAX] {
        q.limit = limit;
        assert!(snapshot.inspect(&q).is_err());
    }
    for scope in [
        "modules",
        "/modules/0/config",
        "/developments/999/definition",
        "/developments/00/definition",
        "/developments/+0/definition",
    ] {
        assert!(snapshot
            .inspect(&query(InspectionSelection::Overview {
                scope: scope.into()
            }))
            .is_err());
    }
    assert!(snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "".into(),
            id: "missing".into()
        }))
        .is_err());
    let command = super::super::RpcCommand::InspectInvention {
        query: query(InspectionSelection::Overview { scope: "".into() }),
    };
    assert!(!command.advances_revision());
    assert_eq!(
        serde_json::from_slice::<super::super::RpcCommand>(&serde_json::to_vec(&command).unwrap())
            .unwrap(),
        command
    );
}

#[test]
fn long_unicode_strings_are_losslessly_readable_in_bounded_fragments() {
    let text = "🎵\n\"".repeat(4000);
    let snapshot = snapshot(json!({"title":text,"modules":[],"connections":[]}));
    let mut q = query(InspectionSelection::Value {
        pointer: "/title".into(),
    });
    q.limit = 3;
    let mut restored = String::new();
    loop {
        let page = snapshot.inspect(&q).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_INSPECTION_BYTES);
        for entry in page.entries {
            assert_eq!(entry.coverage, InspectionCoverage::Fragment);
            let [start, end] = entry.string_range.unwrap();
            assert_eq!(start, restored.len());
            restored.push_str(entry.value.as_ref().unwrap().as_str().unwrap());
            assert_eq!(end, restored.len());
        }
        if let Some(cursor) = page.next_cursor {
            q.cursor = Some(cursor);
        } else {
            break;
        }
    }
    assert_eq!(restored, text);
}

#[test]
fn value_threshold_and_byte_limited_pages_are_exact() {
    for (length, coverage) in [
        (4094, InspectionCoverage::Complete),
        (4095, InspectionCoverage::Omitted),
    ] {
        let snapshot = snapshot(json!({"title":"x".repeat(length),"modules":[],"connections":[]}));
        let page = snapshot
            .inspect(&query(InspectionSelection::Overview { scope: "".into() }))
            .unwrap();
        assert_eq!(
            page.entries
                .iter()
                .find(|e| e.pointer == "/title")
                .unwrap()
                .coverage,
            coverage
        );
    }
    let snapshot = snapshot(
        json!({"modules":[{"id":"voice","type":"oscillator","config":{"values":vec!["x".repeat(3500); 20]}}],"connections":[]}),
    );
    let page = snapshot
        .inspect(&query(InspectionSelection::Value {
            pointer: "/modules/0/config/values".into(),
        }))
        .unwrap();
    assert!(page.entries.len() < 20);
    assert!(page.next_cursor.is_some());
    assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_INSPECTION_BYTES);
}

#[test]
fn nested_module_includes_inherited_development_declaration() {
    let snapshot = snapshot(json!({"modules":[],"connections":[],"developments":[
        {"name":"tone","path":"./tone.json"},
        {"name":"layer","definition":{"modules":[{"id":"osc","type":"tone"}],"connections":[]}}
    ]}));
    for scope in ["/developments/01/definition", "/developments/+1/definition"] {
        assert_eq!(
            snapshot
                .inspect(&query(InspectionSelection::Module {
                    scope: scope.into(),
                    id: "osc".into()
                }))
                .unwrap_err()
                .code,
            RpcErrorCode::InvalidRequest
        );
    }
    let page = snapshot
        .inspect(&query(InspectionSelection::Module {
            scope: "/developments/1/definition".into(),
            id: "osc".into(),
        }))
        .unwrap();
    assert!(page
        .entries
        .iter()
        .any(|e| e.pointer == "/developments/0"
            && e.value.as_ref().unwrap()["path"] == "./tone.json"));
}

#[test]
fn null_and_missing_values_remain_distinct_on_the_wire() {
    for value in [Some(Value::Null), None] {
        let entry = InspectionEntry {
            pointer: "/description".into(),
            coverage: if value.is_some() {
                InspectionCoverage::Complete
            } else {
                InspectionCoverage::Omitted
            },
            value,
            string_range: None,
        };
        let restored: InspectionEntry =
            serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
        assert_eq!(entry, restored);
    }
}

#[test]
fn development_includes_assets_used_by_instance_overrides() {
    let mut snapshot = layered();
    snapshot.document.modules[2].config = json!({"attack":{"$asset":"harmony","path":"/attack"}});
    let page = snapshot
        .inspect(&query(InspectionSelection::Development {
            scope: "".into(),
            name: "voice".into(),
        }))
        .unwrap();
    let mut q = query(page.selection.clone());
    let mut entries = page.entries;
    q.cursor = page.next_cursor;
    while q.cursor.is_some() {
        let page = snapshot.inspect(&q).unwrap();
        entries.extend(page.entries);
        q.cursor = page.next_cursor;
    }
    assert!(entries.iter().any(|e| e.pointer == "/assets/harmony"));
}
