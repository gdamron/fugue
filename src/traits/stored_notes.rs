//! What a module declares about the notes it stores in the document: which
//! control holds them and how each stored value maps to a note. An editor
//! (a piano roll) reads this from the module's description and can then show
//! and change the notes without knowing the module's type.
//!
//! A module that makes its notes as it plays (a melody, a random source)
//! declares nothing: its notes are not stored, so there is nothing to edit.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

type Name = Cow<'static, str>;

/// How a module's stored pattern maps to notes.
///
/// The pattern control's value is JSON text: an array of steps, one per
/// clock step, so a note starts at its step's index and time counts in whole
/// steps. A step is `null` (a rest), a bare integer (a note offset), or an
/// object with the fields named in [`step`](Self::step). A note's pitch is
/// the root control's MIDI note plus its offset. It sounds for its gate
/// length (a ratio of one step) or, followed by held steps, through the end
/// of the last of them. Steps past the array's end, up to the step count,
/// are rests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct StoredNotes {
    /// The control holding the pattern; its config key has the same name.
    pub pattern: Name,
    /// The integer control counting the steps that play.
    pub step_count: Name,
    /// The most steps a pattern holds.
    pub max_steps: u32,
    /// The control holding the MIDI note offsets count from.
    pub root: Name,
    /// The control holding the gate length of a step that names none.
    pub gate_length: Name,
    /// Lowest and highest offset a step can store.
    pub offset_range: [i32; 2],
    /// Lowest and highest MIDI note the module plays; root plus offset is
    /// held to it.
    pub pitch_range: [u8; 2],
    /// The fields of a step object.
    pub step: StepFields,
    /// Set when the module stores a bank of patterns (cells) rather than
    /// one: then [`pattern`](Self::pattern) holds an array of patterns and
    /// one plays at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<StoredCells>,
}

/// How a module that stores a bank of cells plays them, and how one cell is
/// written on its own.
///
/// Cell `N` is written as the control `<cell_key>.N`, with one pattern as
/// JSON text. Such a write changes only that cell, keeps the rest of the
/// bank, and is recorded in the document as the whole bank: a bank the
/// document shared from an asset becomes the module's own copy on its first
/// edit. A cell keeps its length (its trailing rests included), so a write
/// of another length, or to a cell past the bank, is refused. The playing
/// cell's edit is heard from its next step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct StoredCells {
    /// The prefix of the per-cell controls: `cells` names `cells.0`, ...
    pub cell_key: Name,
    /// The most cells a bank holds.
    pub max_cells: u32,
    /// The control reporting the cell that plays now.
    pub playing: Name,
    /// The control selecting the cell to play.
    pub select: Name,
    /// The config key naming the cell that plays first.
    pub start: Name,
    /// The control holding how cells play: `loop` repeats the selected
    /// cell, `one_shot` plays the bank through once.
    pub mode: Name,
    /// The control that, when true, wraps each cycle at the playing cell's
    /// own length rather than the step count.
    pub follow_length: Name,
}

/// The fields of one stored step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct StepFields {
    /// Offset from the root; null or absent is a rest.
    pub note: Name,
    /// How long the note sounds, as a ratio of one step (0 to 1).
    pub gate_length: Name,
    /// `true` continues the previous note through this step without
    /// striking it again; such a step holds no other field.
    pub held: Name,
    /// Strike strength, 0 to 1; absent is full.
    pub velocity: Name,
    /// Grace notes before the note: offsets, played in order. An editor
    /// keeps them with their step.
    pub grace: Name,
}
