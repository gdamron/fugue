//! A running module's description never fails for the size of one control
//! value: a value over the budget, such as a cell sequencer's bank, is left
//! out and named with its size and its cells' lengths, and is read through
//! `get_control`, one cell at a time as `cells.N`. A description with no
//! such value is as it was, byte for byte.

use std::path::Path;
use std::sync::{Arc, Mutex};

use fugue::rpc::{
    DescribeModuleQuery, ModuleDescription, MAX_DESCRIBED_VALUE_BYTES, MAX_DISCOVERY_RESPONSE_BYTES,
};
use fugue::{AudioBackend, ControlValue, Invention, InventionBuilder, RunningInvention};
use serde_json::{json, Value};

const RATE: u32 = 48_000;
type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// A backend that never renders: nothing here needs audio.
struct Silent(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Silent {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

fn start(invention: Invention) -> RunningInvention {
    let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
    let slot = Arc::new(Mutex::new(None));
    runtime.start_with_backend(Silent(slot)).unwrap()
}

fn describe(running: &RunningInvention, module: &str) -> (ModuleDescription, String) {
    let query = DescribeModuleQuery {
        module_id: Some(module.into()),
        ..Default::default()
    };
    let description = running.describe_module(&query).expect("describes");
    let text = serde_json::to_string(&description).unwrap();
    (description, text)
}

fn text(value: ControlValue) -> String {
    match value {
        ControlValue::String(text) => text,
        other => panic!("not text: {other:?}"),
    }
}

/// Checks that `module`'s bank is left out and named, and that each cell
/// reads as its place in the bank, at the length the description gives.
fn assert_bank_left_out(running: &RunningInvention, module: &str) -> Vec<Value> {
    let (description, wire) = describe(running, module);
    assert!(wire.len() <= MAX_DISCOVERY_RESPONSE_BYTES, "{}", wire.len());
    let described: Value = serde_json::from_str(&wire).unwrap();
    assert!(described["control_values"].get("cells").is_none());
    let listed = described["controls"].as_array().unwrap();
    let cells = listed.iter().find(|c| c["key"] == "cells").unwrap();
    assert!(cells.get("default").is_none(), "{cells}");
    assert!(listed
        .iter()
        .all(|c| c["key"] == "cells" || c.get("default").is_some()));

    let bank = text(running.get_control(module, "cells").unwrap());
    let omitted = &description.values_omitted["cells"];
    assert_eq!(
        omitted.bytes as usize,
        serde_json::to_string(&bank).unwrap().len()
    );
    assert!(omitted.bytes as usize > MAX_DESCRIBED_VALUE_BYTES);
    let bank: Vec<Value> = serde_json::from_str(&bank).unwrap();
    let lengths: Vec<u32> = bank
        .iter()
        .map(|c| c.as_array().unwrap().len() as u32)
        .collect();
    assert_eq!(omitted.cell_lengths.as_deref(), Some(&lengths[..]));
    for (index, cell) in bank.iter().enumerate() {
        let read = text(
            running
                .get_control(module, &format!("cells.{index}"))
                .unwrap(),
        );
        assert_eq!(
            &serde_json::from_str::<Value>(&read).unwrap(),
            cell,
            "cells.{index}"
        );
    }

    // Read back, the description serializes as it came.
    let back: ModuleDescription = serde_json::from_str(&wire).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), wire);
    bank
}

/// A piece whose sequencer reads its cells from a score asset: the
/// public-domain BWV 772 score, five cells, copied into a scratch directory.
fn score_piece() -> (tempfile::TempDir, Invention) {
    let dir = tempfile::tempdir().unwrap();
    let score = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/preview_notes/bwv772.score.json");
    std::fs::copy(score, dir.path().join("score.json")).unwrap();
    let piece = json!({
        "version": "1.0.0",
        "assets": {"score": {"path": "score.json"}},
        "modules": [{
            "id": "mel_1",
            "type": "cell_sequencer",
            "config": {"step_count": 704, "cells": {"$asset": "score", "path": "/cells"}},
        }],
        "connections": [],
    });
    let path = dir.path().join("piece.json");
    std::fs::write(&path, piece.to_string()).unwrap();
    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    (dir, invention)
}

#[test]
fn a_score_piece_describes_its_sequencer_and_reads_its_cells_one_at_a_time() {
    let (_dir, invention) = score_piece();
    let running = start(invention);
    let bank = assert_bank_left_out(&running, "mel_1");
    assert_eq!(bank.len(), 5);
    let past = running
        .get_control("mel_1", "cells.5")
        .unwrap_err()
        .to_string();
    assert!(
        past.contains("the bank holds 5 cells, cells.0 to cells.4"),
        "{past}"
    );
    let written = running
        .set_control("mel_1", "cells.5", ControlValue::String("[null]".into()))
        .unwrap_err()
        .to_string();
    assert!(
        written.contains("cells.0 to cells.4"),
        "a write refuses alike: {written}"
    );
    assert!(running.get_control("mel_1", "cells.64").is_err());
}

/// A bank the shape of BWV 772's: five cells of 704 steps, mostly notes.
fn invention_sized_bank() -> Value {
    let cell = |shift: i32| -> Value {
        let steps = (0..704).map(|step| match step % 4 {
            3 => Value::Null,
            1 => json!({"held": true}),
            _ => json!({"note": (step + shift) % 24, "gate_length": 0.92, "velocity": 0.8}),
        });
        Value::Array(steps.collect())
    };
    Value::Array((0..5).map(cell).collect())
}

#[test]
fn a_bank_the_size_of_an_invention_describes_within_the_limit() {
    let bank = invention_sized_bank();
    assert!(serde_json::to_string(&bank).unwrap().len() > 50_000);
    let piece = json!({
        "version": "1.0.0",
        "modules": [{
            "id": "upper",
            "type": "cell_sequencer",
            "config": {"step_count": 704, "cells": bank.to_string()},
        }],
        "connections": [],
    });
    let running = start(serde_json::from_value(piece).unwrap());
    // The bank reads back as the sequencer keeps it (a rest as a step
    // with no note), so its shape is compared.
    let read = assert_bank_left_out(&running, "upper");
    let lengths = |cells: &[Value]| -> Vec<usize> {
        cells
            .iter()
            .map(|cell| cell.as_array().unwrap().len())
            .collect()
    };
    assert_eq!(lengths(&read), lengths(bank.as_array().unwrap()));
}

/// A piece of the small sequencers, each described as before.
fn small_piece() -> Invention {
    serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [
            {"id": "steps", "type": "step_sequencer",
             "config": {"pattern": [0, 4, null, 7], "step_count": 4}},
            {"id": "cells", "type": "cell_sequencer",
             "config": {"cells": [[{"note": 0}, null], [{"note": 7}]], "step_count": 2}},
        ],
        "connections": [],
    }))
    .unwrap()
}

#[test]
fn a_description_with_no_large_value_is_unchanged_byte_for_byte() {
    let running = start(small_piece());
    for module in ["steps", "cells"] {
        let (description, wire) = describe(&running, module);
        assert!(description.values_omitted.is_empty());
        let pinned = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/described")
            .join(format!("{module}.json"));
        let pinned = std::fs::read_to_string(&pinned).unwrap();
        assert_eq!(wire, pinned.trim_end(), "{module}");
    }
}
