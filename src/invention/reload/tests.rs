use super::*;
use crate::invention::builder::InventionBuilder;
use crate::invention::manual_backend::Settled;
use crate::modules::NullBackend;

mod authored;
mod numeric;

fn doc(json: &str) -> Invention {
    Invention::from_json(json).unwrap()
}

/// No declared numeric keys: every value compares as JSON.
fn no_keys(_: &str) -> &'static [crate::module_config::ConfigKey] {
    &[]
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

const DEV_BASE: &str = r#"{
    "version": "1.0.0",
    "developments": [
        {
            "name": "voice",
            "definition": {
                "modules": [
                    { "id": "o", "type": "oscillator", "config": { "waveform": "sine", "frequency": 300.0 } }
                ],
                "connections": [],
                "outputs": [ { "name": "audio", "from": "o", "from_port": "audio" } ]
            }
        }
    ],
    "modules": [
        { "id": "v1", "type": "voice" },
        { "id": "v2", "type": "voice" },
        { "id": "solo", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "v1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "v2", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "solo", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn changed_development_definition_swaps_only_its_instances() {
    let mut running = start(DEV_BASE);
    running
        .snapshot()
        .set_control_with_intent(
            "solo",
            "frequency",
            ControlValue::Number(123.0),
            crate::ControlWriteIntent::Perform,
        )
        .unwrap();

    // Only the development's internal definition changes.
    let report = running
        .reload(doc(&DEV_BASE.replace("300.0", "320.0")))
        .expect("diff applies");

    let mut swapped = report.swapped.clone();
    swapped.sort();
    assert_eq!(swapped, vec!["v1", "v2"]);
    assert!(report.added.is_empty());
    assert!(report.removed.is_empty());
    // solo and dac keep playing; solo keeps its live tweak.
    assert_eq!(report.unchanged, 2);
    assert_eq!(
        running.get_control("solo", "frequency").unwrap(),
        ControlValue::Number(123.0)
    );
}

#[test]
fn unchanged_development_definition_leaves_instances_untouched() {
    let mut running = start(DEV_BASE);

    // Reload the identical document: nothing to do.
    let report = running.reload(doc(DEV_BASE)).expect("diff applies");
    assert!(report.swapped.is_empty());
    assert!(report.added.is_empty());
    assert!(report.removed.is_empty());
    assert!(report.controls_updated.is_empty());
    assert_eq!(report.unchanged, 4);
}

#[test]
fn changed_development_types_are_transitive() {
    let inner = |frequency: f64| {
        format!(
            r#"{{
                "name": "inner",
                "definition": {{
                    "modules": [ {{ "id": "o", "type": "oscillator", "config": {{ "frequency": {frequency} }} }} ],
                    "connections": [],
                    "outputs": [ {{ "name": "audio", "from": "o", "from_port": "audio" }} ]
                }}
            }}"#
        )
    };
    let outer = r#"{
        "name": "outer",
        "definition": {
            "modules": [ { "id": "i", "type": "inner" } ],
            "connections": [],
            "outputs": [ { "name": "audio", "from": "i", "from_port": "audio" } ]
        }
    }"#;
    let document = |frequency: f64| {
        doc(&format!(
            r#"{{
                "version": "1.0.0",
                "developments": [ {}, {} ],
                "modules": [ {{ "id": "dac", "type": "dac" }} ],
                "connections": []
            }}"#,
            inner(frequency),
            outer
        ))
    };

    let previous = DevelopmentDefinitions::resolve(&document(300.0)).unwrap();
    let new = DevelopmentDefinitions::resolve(&document(320.0)).unwrap();

    let changed = changed_development_types(&previous, &new);
    assert!(changed.contains("inner"));
    // outer's own definition is identical, but it instantiates inner.
    assert!(changed.contains("outer"));

    let unchanged = changed_development_types(&previous, &previous.clone());
    assert!(unchanged.is_empty());
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
        plan_reload(&current, &[], &new, &HashSet::new(), no_keys, |_, _| {
            has_control
        })
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

    // A removed key means "back to the default", which needs a rebuild.
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

    let plan = plan_reload(&current, &[], &new, &HashSet::new(), no_keys, |_, _| false).unwrap();
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
        |_, _| false,
    )
    .unwrap();

    assert_eq!(plan.removed, vec!["osc"]);
    // remove_module cleans the connection up; no explicit disconnect planned.
    assert!(plan.removed_connections.is_empty());
    assert!(plan.added_connections.is_empty());
}

/// Checking a reloaded document never builds a running recorder again over
/// the file it is writing.
#[test]
fn reload_never_rebuilds_a_running_recorder_over_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("take.wav");
    let with_recorder = |frequency: &str| {
        BASE.replace(
            r#"{ "id": "dac", "type": "dac" }"#,
            &format!(
                r#"{{ "id": "dac", "type": "dac" }},
                {{ "id": "rec", "type": "audio_file_sink", "config": {{ "path": {path:?} }} }}"#
            ),
        )
        .replace(
            r#"{ "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }"#,
            r#"{ "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" },
            { "from": "osc1", "from_port": "audio", "to": "rec", "to_port": "audio" }"#,
        )
        .replace("440.0", frequency)
    };
    let (runtime, _) = InventionBuilder::new(crate::invention::manual_backend::SAMPLE_RATE)
        .build(doc(&with_recorder("440.0")))
        .unwrap();
    let (mut running, pump) = crate::invention::manual_backend::start_manual(runtime);
    pump.render(2);
    assert!(appears(&path));
    // The recorder keeps writing through its open file; building it again
    // would create the file anew.
    std::fs::remove_file(&path).unwrap();

    running
        .reload(doc(&with_recorder("220.0")))
        .expect("diff applies");
    pump.render(1);
    assert!(
        !path.exists(),
        "the reload rebuilt the recorder over its file"
    );
}

/// Waits, up to two seconds, for a recorder's writer thread to create
/// `path`: it opens the file once the recorder first processes audio.
fn appears(path: &std::path::Path) -> bool {
    for _ in 0..200 {
        if path.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// Counts the live builds of an oscillator; validation builds are not live.
#[derive(Clone, Default)]
struct CountedOscillator(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl crate::ModuleFactory for CountedOscillator {
    fn type_id(&self) -> &'static str {
        "counted_oscillator"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::factory::ModuleBuildResult, Box<dyn std::error::Error>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.build_for_validation(sample_rate, config)
    }

    fn build_for_validation(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::factory::ModuleBuildResult, Box<dyn std::error::Error>> {
        ModuleRegistry::default().build("oscillator", sample_rate, config)
    }
}

/// A reload checks the new document without building any module live: only
/// what the diff adds or rebuilds is built for real.
#[test]
fn reload_checks_the_document_without_live_builds() {
    let counted = CountedOscillator::default();
    let mut registry = ModuleRegistry::default();
    registry.register(counted.clone());
    let with_counted = |frequency: &str| {
        BASE.replace(
            r#"{ "id": "dac", "type": "dac" }"#,
            r#"{ "id": "dac", "type": "dac" }, { "id": "c", "type": "counted_oscillator" }"#,
        )
        .replace("440.0", frequency)
    };
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(doc(&with_counted("440.0")))
        .unwrap();
    let mut running = runtime
        .start_with_backend(NullBackend::new(48_000))
        .unwrap();
    let live = || counted.0.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(live(), 1);

    running
        .reload(doc(&with_counted("220.0")))
        .expect("diff applies");
    assert_eq!(live(), 1, "the reload built an unchanged module live");
}

/// Two siblings each declare a nested `voice` from their own file; editing
/// only the second file rebuilds only the second sibling's instance.
#[test]
fn reload_detects_a_change_to_either_siblings_nested_definition() {
    let dir = tempfile::tempdir().unwrap();
    let voice = |frequency: f64| {
        serde_json::json!({
            "version": "1.0.0",
            "modules": [{ "id": "osc", "type": "oscillator", "config": { "frequency": frequency } }],
            "connections": [],
            "outputs": [{ "name": "audio", "from": "osc", "from_port": "audio" }]
        })
        .to_string()
    };
    let low_path = dir.path().join("low_voice.json");
    let high_path = dir.path().join("high_voice.json");
    std::fs::write(&low_path, voice(110.0)).unwrap();
    std::fs::write(&high_path, voice(880.0)).unwrap();
    let wrapper = |path: &std::path::Path| {
        serde_json::json!({
            "version": "1.0.0",
            "developments": [{ "name": "voice", "path": path }],
            "modules": [{ "id": "v", "type": "voice" }],
            "connections": [],
            "outputs": [{ "name": "audio", "from": "v", "from_port": "audio" }]
        })
    };
    let root = serde_json::json!({
        "version": "1.0.0",
        "developments": [
            { "name": "low", "definition": wrapper(&low_path) },
            { "name": "high", "definition": wrapper(&high_path) }
        ],
        "modules": [{ "id": "a", "type": "low" }, { "id": "b", "type": "high" }],
        "connections": []
    })
    .to_string();
    let mut running = start(&root);

    std::fs::write(&high_path, voice(990.0)).unwrap();
    let report = running.reload(doc(&root)).expect("diff applies");
    assert_eq!(report.swapped, vec!["b"]);
}
