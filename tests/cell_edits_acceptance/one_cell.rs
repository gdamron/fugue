//! Spec point 2: a `cells.N` write replaces cell N only, standalone or in an
//! `apply_edits` batch, keeps trailing rests, and is refused with nothing
//! changed when N is not a cell, the value is not a cell, or its length
//! differs from cell N's.

use fugue::rpc::EditFailureReason;
use serde_json::{json, Value};

use super::{cell, config, looping, notes, three_cells, voice_piece, with_cell, Piece};

fn piece() -> Piece {
    Piece::from_json(&voice_piece(looping(three_cells()), "step"))
}

/// The bank as a sequencer reads it after a whole `cells` write of `bank`.
fn as_written_whole(bank: &Value) -> Value {
    let reference = piece();
    reference
        .write("seq", "cells", bank)
        .expect("a whole write applies");
    reference.read("seq", "cells")
}

#[test]
fn a_one_cell_write_replaces_that_cell_only_standalone_or_in_a_batch() {
    let mut piece = piece();
    let before = piece.read("seq", "cells");
    // Standalone, with trailing rests.
    let second = cell(&[Some(7), None, Some(5), None, None, None, None, None]);
    piece
        .write("seq", "cells.1", &second)
        .expect("cells.1 applies");
    let after = piece.read("seq", "cells");
    assert_eq!(
        (&after[0], &after[2]),
        (&before[0], &before[2]),
        "others unchanged"
    );
    assert_ne!(after[1], before[1], "cell 1 replaced");
    assert_eq!(
        after[1].as_array().unwrap().len(),
        8,
        "trailing rests kept: {}",
        after[1]
    );
    let bank = with_cell(three_cells(), 1, &second);
    assert_eq!(after, as_written_whole(&bank));

    // In a batch.
    let third = notes(&[3; 8]);
    piece
        .edit("seq", "cells.2", &third)
        .expect("the batch applies");
    let after = piece.read("seq", "cells");
    assert_eq!(after, as_written_whole(&with_cell(bank, 2, &third)));
}

#[test]
fn a_one_cell_write_that_does_not_fit_is_refused_with_nothing_changed() {
    let mut piece = piece();
    // A write that fits applies, so the refusals below are this feature's.
    let fits = notes(&[1; 8]);
    piece
        .write("seq", "cells.0", &fits)
        .expect("cells.0 applies");
    let bank = piece.read("seq", "cells");
    let document = piece.document();

    let not_a_cell = json!({ "note": 4 });
    let bad_step = json!([{ "note": "high" }, null, null, null, null, null, null, null]);
    let (short, long) = (notes(&[0, 2, 4, 5]), notes(&[0; 9]));
    let refused: [(&str, &Value, bool); 6] = [
        ("cells.3", &fits, false),  // the bank holds three cells
        ("cells.63", &fits, false), // within the most a bank holds, not this one
        ("cells.0", &not_a_cell, false),
        ("cells.0", &bad_step, false),
        ("cells.1", &short, true),
        ("cells.1", &long, true),
    ];
    for (key, value, names_length) in refused {
        let error = piece
            .write("seq", key, value)
            .expect_err(&format!("{key} = {value}"));
        let batch = piece
            .edit("seq", key, value)
            .expect_err(&format!("batch {key} = {value}"));
        let reason = batch.edit.as_ref().map(|edit| edit.reason);
        assert_eq!(
            reason,
            Some(EditFailureReason::InvalidControlValue),
            "{key}: {batch:?}"
        );
        // Cell 1 holds eight steps; a length refusal says so.
        let named = error.contains('8') && batch.message.contains('8');
        assert!(
            named || !names_length,
            "names the length: {error}; {batch:?}"
        );
    }
    assert_eq!(piece.read("seq", "cells"), bank, "the bank is unchanged");
    let recorded = |doc: &Value| config(doc, "seq", "cells").clone();
    assert_eq!(
        recorded(&piece.document()),
        recorded(&document),
        "the document is unchanged"
    );
}
