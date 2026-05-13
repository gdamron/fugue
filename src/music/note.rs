use std::sync::LazyLock;

/// Precomputed Hz for every MIDI note 0..=127.
///
/// `Note::frequency` does ~600k calls/sec across 13 In C voices on the audio
/// thread; the table replaces a `powf` per call with a single array load.
static MIDI_FREQUENCIES: LazyLock<[f32; 128]> = LazyLock::new(|| {
    let mut table = [0.0f32; 128];
    let mut i = 0;
    while i < 128 {
        table[i] = 440.0 * 2.0_f32.powf((i as f32 - 69.0) / 12.0);
        i += 1;
    }
    table
});

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
    /// Uses A4 (note 69) = 440 Hz as the reference. Backed by a precomputed
    /// 128-entry table so the audio thread does not pay a `powf` per sample.
    pub fn frequency(&self) -> f32 {
        MIDI_FREQUENCIES[self.midi_note.min(127) as usize]
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
