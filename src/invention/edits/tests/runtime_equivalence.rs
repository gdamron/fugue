//! The candidate must match what the runtime's own single commands leave in
//! the retained document, so a save after either path is identical.

use std::collections::HashMap;

use serde_json::json;

use super::*;
use crate::invention::builder::InventionBuilder;
use crate::modules::NullBackend;
use crate::ModuleRegistry;

/// Real registry-backed facts, used to check the candidate against the
/// runtime's own single commands.
struct RegistryFacts {
    registry: ModuleRegistry,
    running: HashMap<String, ModuleFacts>,
}

impl RegistryFacts {
    fn for_document(document: &Invention) -> Self {
        let mut facts = Self {
            registry: ModuleRegistry::default(),
            running: HashMap::new(),
        };
        for spec in &document.modules {
            let built = facts
                .describe(&spec.id, &spec.module_type, &spec.config)
                .unwrap();
            facts.running.insert(spec.id.clone(), built);
        }
        facts
    }
}

impl EditFacts for RegistryFacts {
    fn module(&self, id: &str) -> Option<ModuleFacts> {
        self.running.get(id).cloned()
    }

    fn has_type(&self, module_type: &str) -> bool {
        self.registry.has_type(module_type)
    }

    fn describe(
        &mut self,
        _id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<ModuleFacts, String> {
        let built = self
            .registry
            .for_inspection()
            .build(module_type, 48_000, config)
            .map_err(|error| error.to_string())?;
        Ok(ModuleFacts::from_instance(
            &built.module,
            built.control_surface.as_deref().map(|surface| surface as _),
        ))
    }
}

fn start(document: Invention) -> crate::RunningInvention {
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    runtime
        .start_with_backend(NullBackend::new(48_000))
        .unwrap()
}

fn retained(running: &crate::RunningInvention) -> Invention {
    running.state.lock().unwrap().document().unwrap()
}

#[test]
fn the_candidate_matches_the_equivalent_runtime_commands() {
    let edits = vec![
        add("lfo", "lfo", json!({ "frequency": 2 })),
        connect("lfo", "out", "osc1", "fm"),
        set("osc2", "frequency", ControlValue::String("330".into())),
        set("osc2", "type", ControlValue::String("square".into())),
        set("lfo", "frequency", ControlValue::Number(0.25)),
        disconnect("osc2", "audio", "dac", "audio"),
        remove("osc1"),
        remove("osc2"),
        add("osc2", "lfo", json!(null)),
        connect("osc2", "out", "lfo", "rate"),
    ];

    let running = start(base());
    let before = retained(&running);
    let mut facts = RegistryFacts::for_document(&before);
    let candidate = apply_to_candidate(&before, &edits, &mut facts).expect("the batch applies");

    running
        .add_module("lfo", "lfo", &json!({ "frequency": 2 }))
        .unwrap();
    running.connect("lfo", "out", "osc1", "fm").unwrap();
    running
        .set_control("osc2", "frequency", ControlValue::String("330".into()))
        .unwrap();
    running
        .set_control("osc2", "type", ControlValue::String("square".into()))
        .unwrap();
    running
        .set_control("lfo", "frequency", ControlValue::Number(0.25))
        .unwrap();
    running.disconnect("osc2", "audio", "dac", "audio").unwrap();
    running.remove_module("osc1").unwrap();
    running.remove_module("osc2").unwrap();
    running.add_module("osc2", "lfo", &json!(null)).unwrap();
    running.connect("osc2", "out", "lfo", "rate").unwrap();

    assert_eq!(candidate.document, retained(&running));
    // Only the write to the surviving module remains: the osc2 writes were
    // aimed at an instance removed later in the batch. Its value matches
    // what the runtime applied.
    let [lfo] = candidate.control_writes.as_slice() else {
        panic!("expected one write, got {:?}", candidate.control_writes);
    };
    assert_eq!((lfo.edit_index, lfo.write.module_id.as_str()), (4, "lfo"));
    assert_eq!(
        running.get_control("lfo", "frequency").unwrap(),
        lfo.write.value
    );
}

#[test]
fn registry_facts_read_ports_and_control_kinds() {
    let facts = RegistryFacts::for_document(&base());
    let osc = facts.module("osc1").unwrap();
    assert_eq!(osc.outputs, ["audio"]);
    assert!(osc.inputs.iter().any(|port| port == "fm"));
    assert!(matches!(
        osc.controls.get("frequency"),
        Some(ControlKind::Number { .. })
    ));
    assert!(matches!(
        osc.controls.get("type"),
        Some(ControlKind::String { .. })
    ));
    assert!(facts
        .module("dac")
        .unwrap()
        .inputs
        .iter()
        .any(|port| port == "audio"));
}
