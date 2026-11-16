//! Spec point 1: a cell sequencer's stored-notes declaration names its bank
//! of cells, in its type's description, a running instance's, and the
//! registry's; the step sequencer's declaration is unchanged.

use fugue::rpc::{DescribeModuleQuery, ModuleDescription, RegistryScope};
use fugue::ModuleRegistry;
use serde_json::{json, Value};

use super::{looping, three_cells, voice_piece, Piece, RATE};

fn described(module_type: &str) -> Value {
    let query = DescribeModuleQuery {
        module_type: Some(module_type.into()),
        ..Default::default()
    };
    let registry = ModuleRegistry::default();
    let description =
        ModuleDescription::from_registry(&registry, RATE, RegistryScope::Running, &query).unwrap();
    serde_json::to_value(description).unwrap()
}

/// Checks a cell sequencer's description against the spec, by value.
fn assert_cells_declared(description: &Value) {
    let notes = &description["stored_notes"];
    assert_eq!(
        (&notes["pattern"], &notes["max_steps"]),
        (&json!("cells"), &json!(1024)),
        "{notes:#}"
    );
    let cells = json!({
        "cell_key": "cells", "max_cells": 64, "playing": "cell", "select": "select_cell",
        "start": "start_cell", "mode": "mode", "follow_length": "follow_cell_length",
    });
    assert_eq!(notes["cells"], cells, "{notes:#}");
    // Every existing field is kept, naming the module's own controls.
    let controls = description["controls"].as_array().expect("controls");
    for field in ["step_count", "root", "gate_length"] {
        let named = &notes[field];
        assert!(
            controls.iter().any(|c| &c["key"] == named),
            "{field} names a control: {named}"
        );
    }
    for field in ["offset_range", "pitch_range"] {
        assert_eq!(
            notes[field].as_array().map(Vec::len),
            Some(2),
            "{field}: {notes:#}"
        );
    }
    // A cell holds steps in the step pattern format.
    assert_eq!(
        notes["step"],
        described("step_sequencer")["stored_notes"]["step"]
    );
}

#[test]
fn a_cell_sequencer_describes_its_bank_of_cells() {
    let described = described("cell_sequencer");
    assert_cells_declared(&described);
    let registry = ModuleRegistry::default();
    let declared = serde_json::to_value(registry.stored_notes("cell_sequencer")).unwrap();
    assert_eq!(
        declared, described["stored_notes"],
        "the registry's is the description's"
    );
}

#[test]
fn a_running_cell_sequencer_describes_its_bank_of_cells() {
    let piece = Piece::from_json(&voice_piece(looping(three_cells()), "step"));
    let query = DescribeModuleQuery {
        module_id: Some("seq".into()),
        ..Default::default()
    };
    let description = piece.running.describe_module(&query).expect("describes");
    assert_cells_declared(&serde_json::to_value(description).unwrap());
}

#[test]
fn the_step_sequencer_declares_no_cells_and_is_otherwise_unchanged() {
    let step = json!({
        "note": "note", "gate_length": "gate_length", "held": "held",
        "velocity": "velocity", "grace": "grace",
    });
    let want = json!({
        "pattern": "pattern", "step_count": "step_count", "max_steps": 64, "root": "root_note",
        "gate_length": "gate_length", "offset_range": [-128, 127], "pitch_range": [0, 127],
        "step": step,
    });
    assert_eq!(described("step_sequencer")["stored_notes"], want);
}
