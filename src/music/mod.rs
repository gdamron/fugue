//! Musical scale and note primitives.
//!
//! - [`Note`] - A single pitch as a MIDI note number
//! - [`Scale`] - A root note; degrees are semitone offsets

pub use self::note::Note;

mod note;

/// A musical scale rooted on a given note.
///
/// Degrees are semitone offsets from the root: degree 0 is the root,
/// degree 7 is a perfect fifth, degree 12 is one octave up, etc.
pub struct Scale {
    root: Note,
}

impl Scale {
    /// Creates a new scale with the given root note.
    pub fn new(root: Note) -> Self {
        Self { root }
    }

    /// Returns the note at the given semitone offset from the root.
    ///
    /// Negative degrees go below the root. The result is clamped to MIDI 0-127.
    pub fn get_note(&self, degree: i32) -> Note {
        let midi = (self.root.midi_note as i32 + degree).clamp(0, 127);
        Note::new(midi as u8)
    }
}
