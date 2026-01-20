use std::sync::{Arc, Mutex};

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

/// Control - human input and parameter changes
/// NOT an audio-rate signal - represents user interaction with the system.
///
/// Examples:
/// - Knob positions
/// - Button states
/// - Switch positions  
/// - Key presses
/// - Parameter automation values
/// - MIDI CC values
///
/// These are thread-safe and can be read/written from the audio thread
/// and UI thread simultaneously.
#[derive(Clone)]
pub struct Control<T> {
    value: Arc<Mutex<T>>,
}

impl<T> Control<T> {
    pub fn new(value: T) -> Self {
        Self {
            value: Arc::new(Mutex::new(value)),
        }
    }

    pub fn get(&self) -> T
    where
        T: Copy,
    {
        *self.value.lock().unwrap()
    }

    pub fn set(&self, new_value: T) {
        *self.value.lock().unwrap() = new_value;
    }

    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        f(&*self.value.lock().unwrap())
    }

    pub fn modify<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        f(&mut *self.value.lock().unwrap())
    }

    pub fn inner(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.value)
    }
}

// Legacy type aliases for backward compatibility during migration
// TODO: Remove these once all code is updated
pub type AudioSignal = Audio;
pub type ControlSignal = Audio;
pub type GateSignal = Audio;
pub type TriggerSignal = Audio;

/// ClockSignal - timing information at audio rate
/// Contains beat/measure data updated every sample
#[derive(Debug, Clone, Copy)]
pub struct ClockSignal {
    pub beats: f64,
    pub phase: f32, // 0.0 to 1.0 within current beat
    pub measure: u64,
    pub beat_in_measure: u32,
}

impl ClockSignal {
    pub fn new(beats: f64, phase: f32, measure: u64, beat_in_measure: u32) -> Self {
        Self {
            beats,
            phase: phase.clamp(0.0, 1.0),
            measure,
            beat_in_measure,
        }
    }
}

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
