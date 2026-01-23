use super::Audio;

/// FrequencySignal - pitch information as audio-rate signal
#[derive(Debug, Clone, Copy)]
pub struct FrequencySignal {
    pub hz: f32,
}

impl FrequencySignal {
    pub fn new(hz: f32) -> Self {
        Self { hz }
    }

    pub fn from_midi(midi_note: u8) -> Self {
        let hz = 440.0 * 2.0_f32.powf((midi_note as f32 - 69.0) / 12.0);
        Self { hz }
    }

    pub fn to_audio(&self) -> Audio {
        Audio::new(self.hz)
    }
}
