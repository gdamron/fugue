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

    /// Creates a note from a spelled pitch: step letter, chromatic alteration
    /// (sharps positive, flats negative), and scientific octave — the spelling
    /// used by notation formats like MusicXML (`Eb4` = step 'E', alter -1,
    /// octave 4 = MIDI 63).
    ///
    /// Returns `None` for an invalid step letter or a pitch outside MIDI
    /// 0..=127.
    pub fn from_spelling(step: char, alter: i32, octave: i32) -> Option<Self> {
        let semitone = step_semitone(step)? as i32;
        let midi = (octave + 1) * 12 + semitone + alter;
        u8::try_from(midi).ok().filter(|&m| m <= 127).map(Self::new)
    }
}

/// Semitone offset of a step letter within the octave (C = 0 … B = 11), or
/// `None` if the letter is not a pitch step.
pub fn step_semitone(step: char) -> Option<u8> {
    match step.to_ascii_uppercase() {
        'C' => Some(0),
        'D' => Some(2),
        'E' => Some(4),
        'F' => Some(5),
        'G' => Some(7),
        'A' => Some(9),
        'B' => Some(11),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spells_pitches_to_midi() {
        assert_eq!(Note::from_spelling('C', 0, 4).unwrap().midi_note, 60);
        assert_eq!(Note::from_spelling('E', -1, 4).unwrap().midi_note, 63);
        assert_eq!(Note::from_spelling('F', 1, 5).unwrap().midi_note, 78);
        assert_eq!(Note::from_spelling('A', 0, 0).unwrap().midi_note, 21);
        // Enharmonic spellings land on the same key.
        assert_eq!(
            Note::from_spelling('B', 1, 3).unwrap().midi_note,
            Note::from_spelling('C', 0, 4).unwrap().midi_note
        );
    }

    #[test]
    fn rejects_bad_letters_and_out_of_range_pitches() {
        assert!(Note::from_spelling('H', 0, 4).is_none());
        assert!(Note::from_spelling('C', -1, -1).is_none()); // below MIDI 0
        assert!(Note::from_spelling('G', 1, 9).is_none()); // above MIDI 127
        assert_eq!(step_semitone('g'), Some(7));
        assert_eq!(step_semitone('x'), None);
    }
}
