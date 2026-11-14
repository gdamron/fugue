//! A module's description says how the notes it stores map to notes, so a
//! piano roll can edit a pattern without knowing the module's type.

use super::*;

fn described(module_type: &str) -> serde_json::Value {
    let registry = ModuleRegistry::default();
    let query = DescribeModuleQuery {
        module_type: Some(module_type.to_string()),
        ..Default::default()
    };
    let description =
        ModuleDescription::from_registry(&registry, 48_000, RegistryScope::Running, &query)
            .unwrap();
    serde_json::to_value(description).unwrap()
}

#[test]
fn a_step_sequencer_declares_its_stored_notes() {
    let described = described("step_sequencer");
    assert_eq!(
        described["stored_notes"],
        serde_json::json!({
            "pattern": "pattern",
            "step_count": "step_count",
            "max_steps": 64,
            "root": "root_note",
            "gate_length": "gate_length",
            "offset_range": [-128, 127],
            "pitch_range": [0, 127],
            "step": {
                "note": "note",
                "gate_length": "gate_length",
                "held": "held",
                "velocity": "velocity",
                "grace": "grace"
            }
        })
    );
    let back: crate::StoredNotes =
        serde_json::from_value(described["stored_notes"].clone()).unwrap();
    assert_eq!(back.pattern, "pattern");
}

#[test]
fn a_module_that_makes_its_notes_as_it_plays_declares_none() {
    for module_type in ["melody", "oscillator", "clock"] {
        let described = described(module_type);
        assert!(
            described.get("stored_notes").is_none(),
            "{module_type}: {}",
            described["stored_notes"]
        );
    }
}

#[test]
fn only_the_step_sequencer_stores_notes_among_the_builtin_types() {
    let registry = ModuleRegistry::default();
    let declaring: Vec<&str> = registry
        .types()
        .filter(|name| registry.stored_notes(name).is_some())
        .collect();
    assert_eq!(declaring, ["step_sequencer"]);
}

#[test]
fn a_running_step_sequencer_keeps_its_declaration() {
    let registry = ModuleRegistry::default();
    let snapshot: RuntimeModuleSnapshot = serde_json::from_value(serde_json::json!({
        "info": {"id": "seq", "module_type": "step_sequencer", "config": {}},
        "ports": {"inputs": ["clock", "reset"], "outputs": ["frequency", "gate"]},
    }))
    .unwrap();
    let description = ModuleDescription::from_snapshot(snapshot, 48_000, &registry).unwrap();
    let notes = description.info.stored_notes.expect("declared");
    assert_eq!(
        (notes.pattern.as_ref(), notes.root.as_ref()),
        ("pattern", "root_note")
    );
}
