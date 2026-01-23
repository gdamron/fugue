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
