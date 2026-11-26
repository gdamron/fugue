//! One cell of a cell sequencer written alongside other writes to the same
//! bank: in an `apply_edits` batch each cell write builds on the bank as the
//! batch has it so far, and the batch reports and records the whole bank;
//! performed cell writes made at once from several threads all land; and a
//! cell write is made now, so one asked for at a later time is refused.

use crate::invention::manual_backend::start_manual;
use crate::rpc::{ControlTiming, ControlWhen, StructuralEdit};
use crate::{ControlValue, ControlWrite, ControlWriteIntent, Invention, InventionBuilder};
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

/// A bank of `count` one-step cells, each a rest.
fn rests(count: usize) -> Value {
    Value::Array(vec![json!([null]); count])
}

#[test]
fn performed_cell_writes_made_at_once_all_land() {
    let mut document: Value = serde_json::from_str(&doc()).unwrap();
    document["modules"][1]["config"]["cells"] = rests(16);
    let invention = Invention::from_json(&document.to_string()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    std::thread::scope(|scope| {
        for cell in 0..16 {
            let snapshot = running.snapshot();
            scope.spawn(move || {
                let value = ControlValue::String(json!([{ "note": cell }]).to_string());
                let key = format!("cells.{cell}");
                let written = snapshot.set_control_with_intent(
                    "seq",
                    &key,
                    value,
                    ControlWriteIntent::Perform,
                );
                written.expect("the cell write applies");
            });
        }
    });
    pump.render(2);
    let ControlValue::String(text) = running.get_control("seq", "cells").unwrap() else {
        panic!("cells reads back as text");
    };
    let bank: Value = serde_json::from_str(&text).unwrap();
    for cell in 0..16 {
        assert_eq!(
            bank[cell][0]["note"], cell,
            "cell {cell} kept its write: {bank}"
        );
    }
    running.stop();
}

#[test]
fn a_cell_write_at_a_later_time_is_refused() {
    let invention = Invention::from_json(&doc()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    pump.render(10);
    for intent in [ControlWriteIntent::Perform, ControlWriteIntent::Author] {
        let value = ControlValue::String(json!([{ "note": 5 }, null]).to_string());
        let write = ControlWrite {
            intent,
            timing: ControlTiming {
                when: Some(ControlWhen::NextBar {
                    beats_per_bar: Some(4.0),
                    clock: None,
                }),
                ttl_ms: None,
            },
            ..ControlWrite::new("seq", "cells.1", value)
        };
        let acks = running.write_controls(&[write]);
        let error = acks.error.expect("refused").message;
        assert!(error.contains("timed"), "{intent:?}: {error}");
    }
    let ControlValue::String(text) = running.get_control("seq", "cells").unwrap() else {
        panic!("cells reads back as text");
    };
    let bank: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(bank[1][0]["note"], 7, "unchanged: {bank}");
    running.stop();
}

#[test]
fn a_cell_reads_as_its_place_in_the_bank_now_and_past_the_bank_is_refused() {
    let invention = Invention::from_json(&doc()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let read = |key: &str| match running.get_control("seq", key) {
        Ok(ControlValue::String(text)) => Ok(serde_json::from_str::<Value>(&text).unwrap()),
        other => Err(format!("{other:?}")),
    };
    let bank = read("cells").unwrap();
    assert_eq!(read("cells.1").unwrap(), bank[1]);
    let rests = json!([{"note": 9}, null]);
    running
        .set_control("seq", "cells.1", ControlValue::String(rests.to_string()))
        .unwrap();
    pump.render(1);
    assert_eq!(
        read("cells.1").unwrap(),
        read("cells").unwrap()[1],
        "read after a write"
    );
    assert_ne!(read("cells.1").unwrap(), bank[1]);
    let past = read("cells.2").unwrap_err();
    assert!(
        past.contains("the bank holds 2 cells, cells.0 to cells.1"),
        "{past}"
    );
    running.stop();
}
