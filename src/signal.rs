/// Core signal types in the modular system
/// Like voltage in Eurorack, but typed for different domains

/// Control-rate signal - for modulation, envelopes, LFOs (typically ~1000Hz)
#[derive(Debug, Clone, Copy)]
pub struct ControlSignal {
    pub value: f32,
}

impl ControlSignal {
    pub fn new(value: f32) -> Self {
        Self { value }
    }
}

/// Audio-rate signal - for sound (44.1kHz or 48kHz)
#[derive(Debug, Clone, Copy)]
pub struct AudioSignal {
    pub value: f32,
}

impl AudioSignal {
    pub fn new(value: f32) -> Self {
        Self { value }
    }
    
    pub fn silence() -> Self {
        Self { value: 0.0 }
    }
}

/// Gate signal - for triggers and note on/off (like Eurorack gates)
#[derive(Debug, Clone, Copy)]
pub struct GateSignal {
    pub active: bool,
    pub velocity: f32,  // 0.0 to 1.0
}

impl GateSignal {
    pub fn new(active: bool, velocity: f32) -> Self {
        Self { 
            active, 
            velocity: velocity.clamp(0.0, 1.0),
        }
    }
    
    pub fn off() -> Self {
        Self { active: false, velocity: 0.0 }
    }
}

/// Trigger signal - single-sample pulses (like Eurorack triggers)
#[derive(Debug, Clone, Copy)]
pub struct TriggerSignal {
    pub triggered: bool,
}

impl TriggerSignal {
    pub fn new(triggered: bool) -> Self {
        Self { triggered }
    }
    
    pub fn idle() -> Self {
        Self { triggered: false }
    }
}

/// Clock signal - timing information (beats, measures, phase)
#[derive(Debug, Clone, Copy)]
pub struct ClockSignal {
    pub beats: f64,
    pub phase: f32,      // 0.0 to 1.0 within current beat
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

/// Frequency/Pitch signal - in Hz
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
}
