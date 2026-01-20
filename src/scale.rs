#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Ionian, // Major
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Aeolian, // Natural Minor
    Locrian,
}

impl Mode {
    pub fn intervals(&self) -> &[i32] {
        match self {
            Mode::Ionian => &[0, 2, 4, 5, 7, 9, 11],
            Mode::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Mode::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Mode::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Mode::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Mode::Aeolian => &[0, 2, 3, 5, 7, 8, 10],
            Mode::Locrian => &[0, 1, 3, 5, 6, 8, 10],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub midi_note: u8,
}

impl Note {
    pub fn new(midi_note: u8) -> Self {
        Self { midi_note }
    }

    pub fn frequency(&self) -> f32 {
        440.0 * 2.0_f32.powf((self.midi_note as f32 - 69.0) / 12.0)
    }

    pub fn from_frequency(freq: f32) -> Self {
        let midi = 69.0 + 12.0 * (freq / 440.0).log2();
        Self {
            midi_note: midi.round() as u8,
        }
    }
}

pub struct Scale {
    root: Note,
    mode: Mode,
}

impl Scale {
    pub fn new(root: Note, mode: Mode) -> Self {
        Self { root, mode }
    }

    pub fn get_note(&self, degree: usize) -> Note {
        let intervals = self.mode.intervals();
        let octave = degree / intervals.len();
        let degree_in_octave = degree % intervals.len();

        let midi_note =
            self.root.midi_note as i32 + (octave as i32 * 12) + intervals[degree_in_octave];

        Note::new(midi_note as u8)
    }

    pub fn degrees_in_octave(&self) -> usize {
        self.mode.intervals().len()
    }
}
