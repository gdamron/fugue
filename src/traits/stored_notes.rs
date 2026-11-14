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
