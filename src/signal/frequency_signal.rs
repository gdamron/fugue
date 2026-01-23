use super::Audio;

/// Pitch information represented as frequency in Hz.
#[derive(Debug, Clone, Copy)]
pub struct FrequencySignal {
    /// The frequency in Hz.
    pub hz: f32,
}

impl FrequencySignal {
    /// Creates a new frequency signal with the given Hz value.
    pub fn new(hz: f32) -> Self {
        Self { hz }
    }

    /// Creates a frequency signal from a MIDI note number.
    ///
    /// Uses A4 (note 69) = 440 Hz as the reference.
    pub fn from_midi(midi_note: u8) -> Self {
        let hz = 440.0 * 2.0_f32.powf((midi_note as f32 - 69.0) / 12.0);
        Self { hz }
    }

    /// Converts this frequency signal to an [`Audio`] signal.
    pub fn to_audio(&self) -> Audio {
        Audio::new(self.hz)
    }
}
