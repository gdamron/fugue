//! Musical scale and note primitives.
//!
//! - [`Note`] - A single pitch as a MIDI note number
//! - [`Mode`] - Scale patterns (Ionian, Dorian, etc.)
//! - [`Scale`] - A root note combined with a mode

mod mode;
mod note;

pub use mode::Mode;
pub use note::Note;

/// A musical scale combining a root note with a mode.
///
/// Provides methods to retrieve notes at specific scale degrees,
/// automatically handling octave wrapping.
pub struct Scale {
    root: Note,
    mode: Mode,
}

impl Scale {
    /// Creates a new scale with the given root note and mode.
    pub fn new(root: Note, mode: Mode) -> Self {
        Self { root, mode }
    }

    /// Returns the note at the given scale degree.
    ///
    /// Degree 0 is the root note. Degrees beyond the octave (7+)
    /// automatically wrap into higher octaves.
    pub fn get_note(&self, degree: usize) -> Note {
        let intervals = self.mode.intervals();
        let octave = degree / intervals.len();
        let degree_in_octave = degree % intervals.len();

        let midi_note =
            self.root.midi_note as i32 + (octave as i32 * 12) + intervals[degree_in_octave];

        Note::new(midi_note as u8)
    }

    /// Returns the number of degrees in one octave (typically 7).
    pub fn degrees_in_octave(&self) -> usize {
        self.mode.intervals().len()
    }
}
