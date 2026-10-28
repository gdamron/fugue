//! Reload tests. One that builds a package loader (a sampler) or a
//! development instance and then reloads it holds
//! `pkg::lock::ungoverned_here()` first: a lock another test makes the
//! process's meanwhile would otherwise rebuild the module.

use super::plan::TypeKeys;
use super::*;
use crate::invention::builder::InventionBuilder;
use crate::invention::format::ModuleSpec;
use crate::invention::manual_backend::Settled;
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo};
use crate::modules::NullBackend;
use crate::ControlValue;

mod authored;
mod clock;
mod development_assets;
mod development_definitions;
mod development_events;
mod development_wholes;
mod event_keys;
mod exact_integers;
mod implicit;
mod live_builds;
mod melody;
mod mixer_rebuild;
mod mixer_removal;
mod numeric;
mod relative_assets;
mod removed_keys;
mod replacing;
mod sample_assets;
mod sample_levels;

fn doc(json: &str) -> Invention {
    Invention::from_json(json).unwrap()
}

/// No declared numeric keys: every value compares as JSON.
fn no_keys(_: &str) -> TypeKeys {
    (&[], &[])
}

fn start(json: &str) -> Settled {
    let (runtime, _) = InventionBuilder::new(48_000).build(doc(json)).unwrap();
    Settled::start(runtime)
}

const BASE: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "waveform": "sine", "frequency": 550.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn config_delta_lands_as_control_update_and_survivors_keep_runtime_state() {
    let mut running = start(BASE);
    // Diverge osc2 from its config at runtime with a live (performed)
    // tweak. An authored write would be reverted by the reload instead.
    running
        .snapshot()
        .set_control_with_intent(
            "osc2",
            "frequency",
            ControlValue::Number(111.0),
            crate::ControlWriteIntent::Perform,
        )
        .unwrap();

    // Same document, except osc1's frequency config changed.
    let report = running
        .reload(doc(&BASE.replace("440.0", "220.0")))
        .expect("diff applies");

    assert_eq!(report.controls_updated, vec!["osc1.frequency"]);
    assert!(report.added.is_empty());
    assert!(report.removed.is_empty());
    assert!(report.swapped.is_empty());
    assert_eq!(report.connections_added, 0);
    assert_eq!(report.connections_removed, 0);
    // osc1 (instance kept, control updated), osc2, and dac all survive.
    assert_eq!(report.unchanged, 3);

    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        ControlValue::Number(220.0)
    );
    // The surviving instance keeps its runtime divergence: a rebuild would
    // have reset it to the config value.
    assert_eq!(
        running.get_control("osc2", "frequency").unwrap(),
        ControlValue::Number(111.0)
    );
}

#[test]
fn applied_control_updates_are_not_redetected_by_the_next_reload() {
    let mut running = start(BASE);

    let changed = BASE.replace("440.0", "220.0");
    let report = running.reload(doc(&changed)).expect("diff applies");
    assert_eq!(report.controls_updated, vec!["osc1.frequency"]);

    // Reloading the same document again must be a no-op: the stored config
    // now matches the document, so no delta is re-detected or re-applied
    // (which would stomp any later live tweak of the same control).
    let report = running.reload(doc(&changed)).expect("diff applies");
    assert!(report.controls_updated.is_empty());
    assert_eq!(report.unchanged, 3);
}

#[test]
fn reload_adds_removes_and_swaps_modules() {
    let mut running = start(BASE);

    // osc1's waveform changes (a control key -> written, not rebuilt),
    // osc2 is gone, osc3 arrives with a connection.
    let report = running
        .reload(doc(r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc1", "type": "oscillator", "config": { "waveform": "square", "frequency": 440.0 } },
                { "id": "osc3", "type": "oscillator", "config": { "waveform": "sine", "frequency": 660.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
                { "from": "osc3", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#))
        .expect("diff applies");

    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.controls_updated, ["osc1.waveform"]);
    assert_eq!(report.added, vec!["osc3"]);
    assert_eq!(report.removed, vec!["osc2"]);
    assert_eq!(report.connections_added, 1);
    // osc2's connection disappears with the module, not as an explicit
    // disconnect.
    assert_eq!(report.connections_removed, 0);

    let state = running.state.lock().unwrap();
    assert!(state.modules.contains_key("osc3"));
    assert!(!state.modules.contains_key("osc2"));
    assert_eq!(state.connections.len(), 2);
}

#[test]
fn invalid_document_is_rejected_and_running_graph_is_untouched() {
    let mut running = start(BASE);
    running
        .set_control("osc2", "frequency", ControlValue::Number(111.0))
        .unwrap();

    let error = running
        .reload(doc(r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc1", "type": "no_such_module" },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#))
        .expect_err("unknown module type must be rejected");
    assert!(matches!(error, ReloadError::Invalid(_)));

    // Nothing changed: same modules, and the runtime divergence survives.
    let module_count = running.state.lock().unwrap().modules.len();
    assert_eq!(module_count, 3);
    assert_eq!(
        running.get_control("osc2", "frequency").unwrap(),
        ControlValue::Number(111.0)
    );
}

#[test]
fn config_deltas_that_cannot_be_controls_force_a_swap() {
    let current: IndexMap<String, RuntimeModuleInfo> = [(
        "osc".to_string(),
        RuntimeModuleInfo {
            id: "osc".to_string(),
            module_type: "oscillator".to_string(),
            config: serde_json::json!({ "waveform": "sine", "frequency": 440.0 }),
        },
    )]
    .into_iter()
    .collect();

    let plan_for = |config: serde_json::Value, has_control: bool| {
        let new = Invention {
            modules: vec![ModuleSpec {
                id: "osc".to_string(),
                module_type: "oscillator".to_string(),
                config,
            }],
            ..doc(r#"{ "modules": [], "connections": [] }"#)
        };
        plan_reload(
            &current,
            &[],
            &new,
            &HashSet::new(),
            no_keys,
            |_, _| None,
            |_, _| false,
            |_, _| has_control,
            None,
        )
        .unwrap()
    };

    // A scalar delta on an exposed control key stays live.
    let plan = plan_for(
        serde_json::json!({ "waveform": "sine", "frequency": 220.0 }),
        true,
    );
    assert!(plan.swapped.is_empty());
    assert_eq!(plan.control_updates.len(), 1);

    // The module does not expose the changed key as a control.
    let plan = plan_for(
        serde_json::json!({ "waveform": "sine", "frequency": 220.0 }),
        false,
    );
    assert_eq!(plan.swapped.len(), 1);
    assert!(plan.control_updates.is_empty());

    // A removed key with no fresh value to return to (as an edit batch
    // plans) needs a rebuild.
    let plan = plan_for(serde_json::json!({ "waveform": "sine" }), true);
    assert_eq!(plan.swapped.len(), 1);

    // A non-scalar value cannot be a control write.
    let plan = plan_for(
        serde_json::json!({ "waveform": "sine", "frequency": [220.0] }),
        true,
    );
    assert_eq!(plan.swapped.len(), 1);
}

#[test]
fn null_and_empty_configs_are_equivalent() {
    let current: IndexMap<String, RuntimeModuleInfo> = [(
        "dac".to_string(),
        RuntimeModuleInfo {
            id: "dac".to_string(),
            module_type: "dac".to_string(),
            config: serde_json::Value::Null,
        },
    )]
    .into_iter()
    .collect();

    let new = Invention {
        modules: vec![ModuleSpec {
            id: "dac".to_string(),
            module_type: "dac".to_string(),
            config: serde_json::json!({}),
        }],
        ..doc(r#"{ "modules": [], "connections": [] }"#)
    };

    let plan = plan_reload(
        &current,
        &[],
        &new,
        &HashSet::new(),
        no_keys,
        |_, _| None,
        |_, _| false,
        |_, _| false,
        None,
    )
    .unwrap();
    assert_eq!(plan.unchanged, vec!["dac"]);
    assert!(plan.swapped.is_empty());
}

#[test]
fn connection_diff_skips_endpoints_of_removed_modules() {
    let current: IndexMap<String, RuntimeModuleInfo> = [
        (
            "osc".to_string(),
            RuntimeModuleInfo {
                id: "osc".to_string(),
                module_type: "oscillator".to_string(),
                config: serde_json::Value::Null,
            },
        ),
        (
            "dac".to_string(),
            RuntimeModuleInfo {
                id: "dac".to_string(),
                module_type: "dac".to_string(),
                config: serde_json::Value::Null,
            },
        ),
    ]
    .into_iter()
    .collect();
    let current_connections = vec![RuntimeConnectionInfo {
        from: "osc".to_string(),
        from_port: "audio".to_string(),
        to: "dac".to_string(),
        to_port: "audio".to_string(),
    }];

    let new = doc(r#"{ "modules": [ { "id": "dac", "type": "dac" } ], "connections": [] }"#);
    let plan = plan_reload(
        &current,
        &current_connections,
        &new,
        &HashSet::new(),
        no_keys,
        |_, _| None,
        |_, _| false,
        |_, _| false,
        None,
    )
    .unwrap();

    assert_eq!(plan.removed, vec!["osc"]);
    // remove_module cleans the connection up; no explicit disconnect planned.
    assert!(plan.removed_connections.is_empty());
    assert!(plan.added_connections.is_empty());
}

#[test]
fn a_step_sequencer_reloads_an_out_of_range_whole_number_as_it_loads_it() {
    let document = |root_note: u32, step_count: u32| {
        format!(
            r#"{{"version": "1.0.0", "modules": [
                {{ "id": "seq", "type": "step_sequencer",
                   "config": {{ "root_note": {root_note}, "step_count": {step_count} }} }},
                {{ "id": "dac", "type": "dac" }}
            ], "connections": [
                {{ "from": "seq", "from_port": "frequency", "to": "dac", "to_port": "audio" }}
            ]}}"#
        )
    };
    let mut running = start(&document(48, 16));
    for (root_note, step_count) in [(200, 0), (12, 99)] {
        let loaded = start(&document(root_note, step_count));
        let report = running
            .reload(doc(&document(root_note, step_count)))
            .expect("reloads");
        assert!(report.swapped.is_empty(), "{report:?}");
        for key in ["root_note", "step_count"] {
            assert_eq!(
                running.get_control("seq", key).unwrap(),
                loaded.get_control("seq", key).unwrap(),
                "{key}"
            );
        }
    }
}
