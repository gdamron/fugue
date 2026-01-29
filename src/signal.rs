//! Signal types that flow between modules in the synthesis graph.
//!
//! This module provides the core data types for audio and control signals:
//! - [`Audio`] - Real-time audio-rate signals (waveforms, CV, gates)
//! - [`ClockSignal`] - Timing information (beats, measures)
//! - [`FrequencySignal`] - Pitch information in Hz
//! - [`NoteSignal`] - Combined gate and frequency signal

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

/// Alias for [`Audio`] (deprecated, use `Audio` instead).
pub type AudioSignal = Audio;

/// Timing information updated at audio rate.
///
/// Contains beat and measure data that modules can use for
/// tempo-synchronized behavior.
#[derive(Debug, Clone, Copy)]
pub struct ClockSignal {
    /// Total beats elapsed since the clock started.
    pub beats: f64,
    /// Position within the current beat (0.0 to 1.0).
    pub phase: f32,
    /// Current measure number (zero-indexed).
    pub measure: u64,
    /// Current beat within the measure (zero-indexed).
    pub beat_in_measure: u32,
}

impl ClockSignal {
    /// Creates a new clock signal with the given timing state.
    pub fn new(beats: f64, phase: f32, measure: u64, beat_in_measure: u32) -> Self {
        Self {
            beats,
            phase: phase.clamp(0.0, 1.0),
            measure,
            beat_in_measure,
        }
    }
}

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

/// A combined gate and frequency signal representing a musical note.
///
/// Used to communicate note-on/off state and pitch information
/// between sequencers and voices.
#[derive(Debug, Clone, Copy)]
pub struct NoteSignal {
    /// Gate signal with envelope/velocity information.
    pub gate: Audio,
    /// The pitch of the note.
    pub frequency: FrequencySignal,
}
