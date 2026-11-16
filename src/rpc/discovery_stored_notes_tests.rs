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
fn only_the_sequencers_store_notes_among_the_builtin_types() {
    let registry = ModuleRegistry::default();
    let mut declaring: Vec<&str> = registry
        .types()
        .filter(|name| registry.stored_notes(name).is_some())
        .collect();
    declaring.sort_unstable();
    assert_eq!(declaring, ["cell_sequencer", "step_sequencer"]);
}

#[test]
fn a_cell_sequencer_declares_its_bank_and_how_its_cells_play() {
    let described = described("cell_sequencer");
    let notes = &described["stored_notes"];
    assert_eq!(
        (&notes["pattern"], &notes["max_steps"], &notes["root"]),
        (&"cells".into(), &1024.into(), &"root_note".into())
    );
    assert_eq!(
        notes["cells"],
        serde_json::json!({
            "cell_key": "cells",
            "max_cells": 64,
            "playing": "cell",
            "select": "select_cell",
            "start": "start_cell",
            "mode": "mode",
            "follow_length": "follow_cell_length"
        })
    );
    let back: crate::StoredNotes = serde_json::from_value(notes.clone()).unwrap();
    assert_eq!(back.cells.map(|cells| cells.max_cells), Some(64));
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

/// A cell sequencer whose type defaults hold a bank too large to describe.
/// A cell sequencer type whose default bank holds `.1` cells of 1024 steps.
struct LargeBank(&'static str, usize);

impl crate::ModuleFactory for LargeBank {
    fn type_id(&self) -> &'static str {
        self.0
    }
    fn stored_notes(&self) -> Option<crate::StoredNotes> {
        ModuleRegistry::default().stored_notes("cell_sequencer")
    }
    fn build(
        &self,
        sample_rate: u32,
        _: &serde_json::Value,
    ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
        let note = |n: i32| serde_json::json!({"note": n % 12, "velocity": 0.5});
        let cell: Vec<_> = (0..1024).map(note).collect();
        let config = serde_json::json!({"cells": vec![cell; self.1], "step_count": 1024});
        ModuleRegistry::default().build("cell_sequencer", sample_rate, &config)
    }
}

#[test]
fn a_type_described_leaves_a_large_default_out_and_a_catalog_keeps_it() {
    let mut registry = ModuleRegistry::default();
    registry.register(LargeBank("large_bank", 1));
    let query = DescribeModuleQuery {
        module_type: Some("large_bank".to_string()),
        ..Default::default()
    };
    let described =
        ModuleDescription::from_registry(&registry, 48_000, RegistryScope::Running, &query)
            .unwrap();
    assert_eq!(
        described.values_omitted["cells"].cell_lengths,
        Some(vec![1024])
    );
    let catalog = ModuleTypeList::from_registry(
        &registry,
        48_000,
        RegistryScope::Running,
        &ModuleTypeQuery {
            types: Some(vec!["large_bank".to_string()]),
            detail: TypeDetail::Full,
        },
    )
    .unwrap();
    let Some([ModuleTypeDetail::Available { info }]) = catalog.details.as_deref() else {
        panic!("{catalog:?}");
    };
    let cells = info.controls.iter().find(|c| c.key == "cells").unwrap();
    let bank = cells.default.as_string().unwrap();
    assert!(
        bank.len() > super::super::MAX_DESCRIBED_VALUE_BYTES,
        "kept whole"
    );
}

#[test]
fn a_type_too_large_to_list_whole_is_unavailable_and_the_catalog_lists_the_rest() {
    let mut registry = ModuleRegistry::default();
    registry.register(LargeBank("huge_bank", 3));
    let catalog = ModuleTypeList::from_registry(
        &registry,
        48_000,
        RegistryScope::Running,
        &ModuleTypeQuery {
            types: Some(vec!["huge_bank".to_string(), "oscillator".to_string()]),
            detail: TypeDetail::Full,
        },
    )
    .unwrap();
    let details = catalog.details.as_deref().unwrap();
    assert!(matches!(
        &details[0],
        ModuleTypeDetail::Unavailable { type_name, error }
            if type_name == "huge_bank" && error.code == RpcErrorCode::ResponseTooLarge
    ));
    assert!(matches!(&details[1], ModuleTypeDetail::Available { .. }));
}

#[test]
fn a_description_leaving_a_value_out_writes_its_numbers_as_before() {
    let mut registry = ModuleRegistry::default();
    registry.register(LargeBank("large_bank", 1));
    let query = DescribeModuleQuery {
        module_type: Some("large_bank".to_string()),
        ..Default::default()
    };
    let mut described =
        ModuleDescription::from_registry(&registry, 48_000, RegistryScope::Running, &query)
            .unwrap();
    assert!(!described.values_omitted.is_empty());
    let gate = (described.info.controls.iter_mut())
        .find(|c| c.key == "gate_length")
        .unwrap();
    gate.default = crate::ControlValue::Number(0.1);
    described
        .control_values
        .insert("gate_length".into(), crate::ControlValue::Number(0.1));
    let wire = serde_json::to_string(&described).unwrap();
    assert!(!wire.contains("0.10000000149011612"), "widened");
    let wire: serde_json::Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(wire["control_values"]["gate_length"].to_string(), "0.1");
}
