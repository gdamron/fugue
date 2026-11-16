//! One cell written on its own: `cells.N` replaces cell N of the bank and
//! is made as a write of the whole bank, built here on a control thread
//! from the bank the sequencer reads back now. The sequencer takes it as it
//! takes any written bank, whole, so nothing it plays restarts.
//!
//! A cell keeps its length: how long a cell is decides how its cycle
//! lines up with the piece (a cell played for its own length, a conductor
//! that sets the step count from it), and changing that is choosing how
//! cells repeat, which a note edit never does. Its trailing rests are kept.
//!
//! `cells.N` reads as cell N of the bank the sequencer reads back now, so
//! a reader can take one cell of a bank too large to read whole.

use super::parse::parse_sequence_bank_json;
use super::parse_pattern as parse_cell;
use super::table::bank_json;
use super::{MAX_SEQUENCES, MAX_STEPS};
use crate::{ControlValue, StepFields, StoredCells, StoredNotes};

/// The notes the sequencer stores: its `cells` bank, each cell a pattern of
/// steps that are note offsets from `root_note`, one cell playing at a time.
pub(super) fn stored_notes() -> StoredNotes {
    use std::borrow::Cow::Borrowed;
    StoredNotes {
        pattern: Borrowed("cells"),
        step_count: Borrowed("step_count"),
        max_steps: MAX_STEPS as u32,
        root: Borrowed("root_note"),
        gate_length: Borrowed("gate_length"),
        offset_range: [i8::MIN.into(), i8::MAX.into()],
        pitch_range: [0, 127],
        step: StepFields {
            note: Borrowed("note"),
            gate_length: Borrowed("gate_length"),
            held: Borrowed("held"),
            velocity: Borrowed("velocity"),
            grace: Borrowed("grace"),
        },
        cells: Some(StoredCells {
            cell_key: Borrowed("cells"),
            max_cells: MAX_SEQUENCES as u32,
            playing: Borrowed("cell"),
            select: Borrowed("select_cell"),
            start: Borrowed("start_cell"),
            mode: Borrowed("mode"),
            follow_length: Borrowed("follow_cell_length"),
        }),
    }
}

/// The bank `whole` reads back as, with cell `position` replaced by
/// `cell`, as JSON text; or why the cell does not fit.
pub(super) fn assemble_cell(
    whole: &ControlValue,
    position: u16,
    cell: &ControlValue,
) -> Result<ControlValue, String> {
    let index = usize::from(position);
    let mut bank = parse_sequence_bank_json(whole.as_string()?)?;
    let Some(current) = bank.get(index) else {
        return Err(past_bank(index, bank.len()));
    };
    let value: serde_json::Value = serde_json::from_str(cell.as_string()?)
        .map_err(|error| format!("cells.{index}: {error}"))?;
    if !value.is_array() {
        return Err(format!("cells.{index}: a cell is an array of steps"));
    }
    let replaced =
        parse_cell(Some(&value), &format!("cells[{index}]")).map_err(|e| e.to_string())?;
    let (length, written) = (current.len(), replaced.len());
    if written != length {
        return Err(format!(
            "cells.{index} is {length} steps long and keeps its length; this cell has {written}"
        ));
    }
    bank[index] = replaced;
    Ok(ControlValue::String(bank_json(&bank)))
}

/// Cell `position` of the bank `whole` reads back as, as JSON text; or,
/// as a write to it would be, refused when the bank holds no such cell.
pub(super) fn read_cell(whole: &ControlValue, position: u16) -> Result<ControlValue, String> {
    let index = usize::from(position);
    let bank = parse_sequence_bank_json(whole.as_string()?)?;
    let cell = bank
        .get(index)
        .ok_or_else(|| past_bank(index, bank.len()))?;
    let text = serde_json::to_string(cell).map_err(|error| error.to_string())?;
    Ok(ControlValue::String(text))
}

/// Why `cells.{index}` names no cell of a bank of `count`.
fn past_bank(index: usize, count: usize) -> String {
    match count {
        0 => format!("cells.{index}: the bank holds no cells"),
        1 => format!("cells.{index}: the bank holds 1 cell, cells.0"),
        _ => format!(
            "cells.{index}: the bank holds {count} cells, cells.0 to cells.{}",
            count - 1
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> ControlValue {
        ControlValue::String(value.to_string())
    }

    const BANK: &str = r#"[[{"note":0},{"held":true},null,null],[{"note":7}]]"#;

    fn bank(value: &ControlValue) -> serde_json::Value {
        serde_json::from_str(value.as_string().unwrap()).unwrap()
    }

    #[test]
    fn a_cell_replaces_its_place_and_keeps_its_trailing_rests() {
        let written =
            assemble_cell(&text(BANK), 0, &text(r#"[{"note":2},null,null,null]"#)).unwrap();
        let expected = parse_sequence_bank_json(r#"[[{"note":2},null,null,null],[{"note":7}]]"#);
        assert_eq!(bank(&written), bank(&text(&bank_json(&expected.unwrap()))));
        assert_eq!(bank(&written)[0].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_cell_of_another_length_is_refused_naming_the_length() {
        let longer = assemble_cell(&text(BANK), 1, &text(r#"[{"note":7},{"note":9}]"#));
        let error = longer.unwrap_err();
        assert!(
            error.contains("1 steps long") && error.contains("has 2"),
            "{error}"
        );
        let shorter = assemble_cell(&text(BANK), 0, &text(r#"[{"note":0}]"#));
        assert!(shorter.unwrap_err().contains("4 steps long"));
    }

    #[test]
    fn a_cell_reads_as_its_place_in_the_bank_and_past_the_bank_is_refused() {
        let read = read_cell(&text(BANK), 0).unwrap();
        let cell: serde_json::Value = serde_json::from_str(read.as_string().unwrap()).unwrap();
        assert_eq!(
            cell,
            bank(&text(&bank_json(&parse_sequence_bank_json(BANK).unwrap())))[0]
        );
        let past = read_cell(&text(BANK), 2).unwrap_err();
        let written = assemble_cell(&text(BANK), 2, &text("[null]")).unwrap_err();
        assert_eq!(past, written, "the read refuses as the write does");
        assert!(read_cell(&ControlValue::Number(1.0), 0).is_err());
    }

    #[test]
    fn a_cell_past_the_bank_or_not_a_cell_is_refused() {
        let past = assemble_cell(&text(BANK), 2, &text(r#"[{"note":0}]"#)).unwrap_err();
        assert!(past.contains("cells.0 to cells.1"), "{past}");
        let object = assemble_cell(&text(BANK), 1, &text(r#"{"note":0}"#)).unwrap_err();
        assert!(object.contains("array of steps"), "{object}");
        let bad = assemble_cell(&text(BANK), 1, &text(r#"[{"note":999}]"#)).unwrap_err();
        assert!(bad.contains("cells[1]"), "{bad}");
        assert!(assemble_cell(&text(BANK), 1, &text("not json")).is_err());
        assert!(assemble_cell(&text(BANK), 1, &ControlValue::Number(1.0)).is_err());
    }
}
