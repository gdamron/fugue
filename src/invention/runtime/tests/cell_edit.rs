//! One cell of a cell sequencer written in an `apply_edits` batch with other
//! writes to the same bank: each cell write builds on the bank as the batch
//! has it so far, so none undoes another, and the batch reports and records
//! the whole bank.

use crate::invention::manual_backend::start_manual;
use crate::rpc::StructuralEdit;
use crate::{ControlValue, Invention, InventionBuilder};
use serde_json::{json, Value};

const BANK: &str = r#"[[{"note":0},{"note":2},null,null],[{"note":7},null]]"#;

fn doc() -> String {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 240 } },
            { "id": "seq", "type": "cell_sequencer",
              "config": { "step_count": 4, "cells": serde_json::from_str::<Value>(BANK).unwrap() } },
            { "id": "dac", "type": "dac" },
        ],
        "connections": [
            { "from": "clock", "from_port": "beat_x4", "to": "seq", "to_port": "clock" },
            { "from": "seq", "from_port": "gate", "to": "dac", "to_port": "audio_left" },
        ],
    })
    .to_string()
}

fn write(module: &str, key: &str, value: Value) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: module.into(),
        key: key.into(),
        value: ControlValue::String(value.to_string()),
    }
}

/// Applies `edits` to the running piece; returns the bank `seq` reads back,
/// the bank its document records, and the keys the report says were written.
fn applied(edits: &[StructuralEdit], module: &str) -> (Value, Value, Vec<String>) {
    let invention = Invention::from_json(&doc()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let report = running.apply_edits(edits).expect("the batch commits");
    pump.render(2);
    let shown = match running.get_control(module, "cells").unwrap() {
        ControlValue::String(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("{other:?}"),
    };
    let document = running.document().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == module);
    let recorded = spec.unwrap().config["cells"]
        .as_str()
        .map(serde_json::from_str);
    let keys = (report.controls_written.iter())
        .map(|written| format!("{}.{}", written.module_id, written.key))
        .collect();
    running.stop();
    (shown, recorded.expect("recorded as text").unwrap(), keys)
}

#[test]
fn two_cells_written_in_one_batch_both_land() {
    let edits = [
        write("seq", "cells.0", json!([{ "note": 4 }, null, null, null])),
        write("seq", "cells.1", json!([{ "note": 9 }, null])),
    ];
    let (shown, recorded, keys) = applied(&edits, "seq");
    assert_eq!(shown[0][0]["note"], 4, "{shown}");
    assert_eq!(shown[1][0]["note"], 9, "{shown}");
    assert_eq!(recorded, shown);
    assert_eq!(keys, ["seq.cells"]);
}

#[test]
fn a_cell_builds_on_a_bank_written_earlier_in_the_batch() {
    let edits = [
        write(
            "seq",
            "cells",
            json!([[{ "note": 1 }, null], [{ "note": 3 }]]),
        ),
        write("seq", "cells.1", json!([{ "note": 5 }])),
    ];
    let (shown, _, _) = applied(&edits, "seq");
    assert_eq!(shown.as_array().unwrap().len(), 2);
    assert_eq!(
        (&shown[0][0]["note"], &shown[1][0]["note"]),
        (&json!(1), &json!(5))
    );
}

#[test]
fn a_cell_of_a_sequencer_added_in_the_batch_is_written() {
    let edits = [
        StructuralEdit::AddModule {
            id: "added".into(),
            module_type: "cell_sequencer".into(),
            config: json!({ "cells": serde_json::from_str::<Value>(BANK).unwrap() }),
            name: None,
        },
        write("added", "cells.1", json!([{ "note": 11 }, null])),
    ];
    let (shown, recorded, _) = applied(&edits, "added");
    assert_eq!(shown[1][0]["note"], 11, "{shown}");
    assert_eq!(shown[0][1]["note"], 2, "cell 0 kept: {shown}");
    assert_eq!(recorded, shown);
}
