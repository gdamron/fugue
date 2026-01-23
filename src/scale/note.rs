/// A musical note represented as a MIDI note number.
///
/// MIDI note numbers range from 0-127, where 60 is middle C (C4)
/// and 69 is A4 (440 Hz).
#[derive(Debug, Clone, Copy)]
pub struct Note {
    /// The MIDI note number (0-127).
    pub midi_note: u8,
}

impl Note {
    /// Creates a note from a MIDI note number.
    pub fn new(midi_note: u8) -> Self {
        Self { midi_note }
    }

    /// Returns the frequency of this note in Hz.
    ///
    /// Uses A4 (note 69) = 440 Hz as the reference.
    pub fn frequency(&self) -> f32 {
        440.0 * 2.0_f32.powf((self.midi_note as f32 - 69.0) / 12.0)
    }

    /// Creates a note from a frequency in Hz.
    ///
    /// The frequency is rounded to the nearest MIDI note.
    pub fn from_frequency(freq: f32) -> Self {
        let midi = 69.0 + 12.0 * (freq / 440.0).log2();
        Self {
            midi_note: midi.round() as u8,
        }
    }
}
