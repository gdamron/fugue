//! Spec point 3: the first `cells.N` write to a sequencer whose cells are an
//! `$asset` reference copies the whole bank into the piece, with cell N
//! changed; other users of the asset keep the reference and the original
//! cells; the asset file is never written; later edits change the copy; and
//! a saved piece reloads and plays the edited cells.
//!
//! The piece plays the cells of Bach's public-domain Invention 1, BWV 772
//! (`tests/fixtures/preview_notes/bwv772.score.json`), copied into a scratch
//! directory.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{config, differences, wire, Piece};

fn asset_ref() -> Value {
    json!({ "$asset": "score", "path": "/cells" })
}

/// Two sequencers on one clock, `mel_a` playing `a_cells` and `mel_b`
/// `b_cells`, each heard as a scaled-down tap of its frequency.
fn piece_doc(a_cells: Value, b_cells: Value) -> Value {
    let seq = |cells: Value| json!({ "step_count": 24, "start_cell": 0, "cells": cells });
    let tap = json!({ "level": 0.001 });
    json!({
        "version": "1.0.0",
        "assets": { "score": { "path": "score.json" } },
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 240 } },
            { "id": "mel_a", "type": "cell_sequencer", "config": seq(a_cells) },
            { "id": "mel_b", "type": "cell_sequencer", "config": seq(b_cells) },
            { "id": "tap_a", "type": "vca", "config": tap },
            { "id": "tap_b", "type": "vca", "config": tap },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [
            wire("clock", "beat_x4", "mel_a", "clock"),
            wire("clock", "beat_x4", "mel_b", "clock"),
            wire("mel_a", "frequency", "tap_a", "audio"),
            wire("mel_b", "frequency", "tap_b", "audio"),
            wire("tap_a", "audio", "dac", "audio_left"),
            wire("tap_b", "audio", "dac", "audio_right"),
        ],
    })
}

/// A scratch directory holding the score and `piece.json`, whose two
/// sequencers both refer to the score's cells.
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Scratch {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let score = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/preview_notes/bwv772.score.json");
        std::fs::copy(score, root.join("score.json")).unwrap();
        let scratch = Scratch { _dir: dir, root };
        scratch.write("piece.json", &piece_doc(asset_ref(), asset_ref()));
        scratch
    }

    fn write(&self, name: &str, doc: &Value) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(doc).unwrap()).unwrap();
        path
    }

    fn piece(&self) -> Piece {
        Piece::from_file(&self.root.join("piece.json"))
    }

    fn score_bytes(&self) -> Vec<u8> {
        std::fs::read(self.root.join("score.json")).unwrap()
    }

    /// The score's cells, each cell `(index, note)` names starting on `note`.
    fn cells(&self, edits: &[(usize, i32)]) -> Value {
        let score: Value = serde_json::from_slice(&self.score_bytes()).unwrap();
        let mut cells = score["cells"].clone();
        for (index, note) in edits {
            cells[*index][0] = json!({ "note": note });
        }
        cells
    }

    /// The bank a sequencer reads as once `recorded` (an array, or JSON text
    /// of one) is written to it whole: two recordings compare by what they
    /// play, not by how they spell it.
    fn as_played(&self, recorded: &Value) -> Value {
        let bank = match recorded {
            Value::String(text) => serde_json::from_str(text).expect("bank text parses"),
            other => other.clone(),
        };
        assert!(
            bank.is_array(),
            "the piece holds the bank itself: {recorded}"
        );
        let reference = self.piece();
        reference
            .write("mel_a", "cells", &bank)
            .expect("a whole write applies");
        reference.read("mel_a", "cells")
    }
}

#[test]
fn the_first_cell_edit_copies_the_bank_into_the_piece_and_later_edits_change_the_copy() {
    let scratch = Scratch::new();
    let score_before = scratch.score_bytes();
    let mut piece = scratch.piece();
    let b_before = piece.read("mel_b", "cells");
    assert_eq!(
        b_before.as_array().map(Vec::len),
        Some(5),
        "the score's cells load"
    );

    // Standalone first, then through a batch, which changes the copy.
    let second = scratch.cells(&[(1, 7)]);
    piece
        .write("mel_a", "cells.1", &second[1])
        .expect("cells.1 applies");
    let both = scratch.cells(&[(0, 2), (1, 7)]);
    for (bank, edit) in [(second, None), (both.clone(), Some(&both[0]))] {
        if let Some(first) = edit {
            piece
                .edit("mel_a", "cells.0", first)
                .expect("the batch applies");
        }
        let played = piece.read("mel_a", "cells");
        assert_eq!(
            played,
            scratch.as_played(&bank),
            "the module plays the edited bank"
        );
        let document = piece.document();
        let recorded = config(&document, "mel_a", "cells");
        assert!(
            !recorded.to_string().contains("$asset"),
            "no reference left: {recorded}"
        );
        assert_eq!(
            scratch.as_played(recorded),
            played,
            "the document holds the edited bank"
        );
        // The other user of the asset keeps the reference and the original.
        assert_eq!(config(&document, "mel_b", "cells"), &asset_ref());
        assert_eq!(piece.read("mel_b", "cells"), b_before);
        assert_eq!(
            scratch.score_bytes(),
            score_before,
            "the asset file is unchanged"
        );
    }
}

#[test]
fn a_saved_piece_reloads_and_plays_the_edited_cells() {
    let scratch = Scratch::new();
    let score_before = scratch.score_bytes();
    let piece = scratch.piece();
    let b_before = piece.read("mel_b", "cells");
    let edited = scratch.cells(&[(0, 5)]);
    piece
        .write("mel_a", "cells.0", &edited[0])
        .expect("cells.0 applies");
    let played = piece.read("mel_a", "cells");

    // Both ways out: the save, and the snapshot document written as a file.
    let saved = scratch.root.join("saved.json");
    let to_save = piece
        .running
        .document_to_save()
        .expect("a document to save");
    to_save.save_to_file(&saved).expect("saves");
    let snapshot = scratch.write("snapshot.json", &piece.document());
    assert_eq!(
        scratch.score_bytes(),
        score_before,
        "the asset file is unchanged"
    );

    let expected = scratch.write("expected.json", &piece_doc(edited, asset_ref()));
    let want = Piece::from_file(&expected).render(1200);
    for path in [saved, snapshot] {
        let reloaded = Piece::from_file(&path);
        assert_eq!(
            reloaded.read("mel_a", "cells"),
            played,
            "{}",
            path.display()
        );
        assert_eq!(
            reloaded.read("mel_b", "cells"),
            b_before,
            "{}",
            path.display()
        );
        let heard = Piece::from_file(&path).render(1200);
        assert_eq!(
            differences(&heard.0, &want.0),
            None,
            "{} plays the edit",
            path.display()
        );
        assert_eq!(differences(&heard.1, &want.1), None, "{}", path.display());
    }
}
