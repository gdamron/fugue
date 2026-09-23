//! Effective dependency context must agree with runtime registration.
use super::*;

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
fn dependency_context_matches_builder_precedence_for_local_alias_collisions() {
    for ancestor_visible in [true, false] {
        let ancestor = json!({"name":"tone","definition":{
            "modules":[{"id":"osc","type":"oscillator"}],"connections":[],
            "controls":[{"key":"pitch","module":"osc","control":"frequency"}]
        }});
        let key = if ancestor_visible {
            "pitch"
        } else {
            "local_pitch"
        };
        let layer = json!({"name":"layer","definition":{
            "developments":[{"name":"tone","definition":{
                "modules":[{"id":"local_osc","type":"oscillator"}],"connections":[],
                "controls":[{"key":"local_pitch","module":"local_osc","control":"frequency"}]
            }}],
            "modules":[{"id":"voice","type":"tone","config":{(key):220}}],"connections":[]
        }});
        let definitions = if ancestor_visible {
            vec![ancestor, layer]
        } else {
            vec![layer, ancestor]
        };
        let snapshot = snapshot(json!({"developments":definitions,
            "modules":[{"id":"layer_1","type":"layer"}],"connections":[]}));
        // Only the effective factory accepts the configured control key.
        crate::InventionBuilder::new(48_000)
            .build(snapshot.document.clone())
            .unwrap();
        let scope = if ancestor_visible {
            "/developments/1/definition"
        } else {
            "/developments/0/definition"
        };
        let local_pointer = format!("{scope}/developments/0");
        let expected = if ancestor_visible {
            "/developments/0"
        } else {
            &local_pointer
        };
        let page = snapshot
            .inspect(&query(InspectionSelection::Module {
                scope: scope.into(),
                id: "voice".into(),
            }))
            .unwrap();
        let dependencies: Vec<_> = page
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .value
                    .as_ref()
                    .is_some_and(|value| value["name"] == "tone")
            })
            .map(|entry| entry.pointer.as_str())
            .collect();
        assert_eq!(dependencies, vec![expected]);
        // The unused local declaration still belongs in the authored inventory.
        let inventory = snapshot
            .inspect(&query(InspectionSelection::Overview {
                scope: scope.into(),
            }))
            .unwrap();
        assert!(inventory
            .entries
            .iter()
            .any(|entry| entry.pointer == local_pointer));
    }
}
