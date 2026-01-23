/// Audio - real-time audio-rate signal flowing through modules
/// This is the metaphorical "cable" signal in modular synthesis.
/// Represents a single value per sample at audio rate (e.g., 44.1kHz).
///
/// Can carry different types of information:
/// - Sound waveforms
/// - Control voltages (CV)
/// - Gates/triggers (0.0 = off, >0.0 = on with velocity)
/// - Pitch information (in Hz or as CV)
/// - LFO/envelope signals
///
/// In Eurorack terms, everything that flows through patch cables is Audio.
#[derive(Debug, Clone, Copy)]
pub struct Audio {
    pub value: f32,
}

impl Audio {
    pub fn new(value: f32) -> Self {
        Self { value }
    }

    pub fn silence() -> Self {
        Self { value: 0.0 }
    }

    /// Create a gate signal (0.0 or velocity value)
    pub fn gate(active: bool, velocity: f32) -> Self {
        Self {
            value: if active {
                velocity.clamp(0.0, 1.0)
            } else {
                0.0
            },
        }
    }

    /// Convert MIDI note to frequency in Hz
    pub fn from_midi(midi_note: u8) -> Self {
        let hz = 440.0 * 2.0_f32.powf((midi_note as f32 - 69.0) / 12.0);
        Self { value: hz }
    }
}
