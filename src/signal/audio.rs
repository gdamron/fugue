/// A real-time audio-rate signal that flows through modules.
///
/// This is the metaphorical "cable" signal in modular synthesis,
/// representing a single value per sample at audio rate (e.g., 44.1kHz).
///
/// Can carry different types of information:
/// - Sound waveforms
/// - Control voltages (CV)
/// - Gates/triggers (0.0 = off, >0.0 = on with velocity)
/// - Pitch information (in Hz or as CV)
/// - LFO/envelope signals
#[derive(Debug, Clone, Copy)]
pub struct Audio {
    /// The signal value, typically in the range -1.0 to 1.0 for audio.
    pub value: f32,
}

impl Audio {
    /// Creates a new audio signal with the given value.
    pub fn new(value: f32) -> Self {
        Self { value }
    }

    /// Creates a silent audio signal (value = 0.0).
    pub fn silence() -> Self {
        Self { value: 0.0 }
    }

    /// Creates a gate signal for triggering envelopes or events.
    ///
    /// Returns a signal with the velocity value (clamped to 0.0-1.0) when active,
    /// or 0.0 when inactive.
    pub fn gate(active: bool, velocity: f32) -> Self {
        Self {
            value: if active {
                velocity.clamp(0.0, 1.0)
            } else {
                0.0
            },
        }
    }

    /// Creates an audio signal from a MIDI note number.
    ///
    /// Converts the MIDI note to its corresponding frequency in Hz
    /// using A4 (note 69) = 440 Hz as the reference.
    pub fn from_midi(midi_note: u8) -> Self {
        let hz = 440.0 * 2.0_f32.powf((midi_note as f32 - 69.0) / 12.0);
        Self { value: hz }
    }
}
