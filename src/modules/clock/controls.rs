//! Thread-safe controls for the Clock module.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

/// Thread-safe controls for the Clock module.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
///
/// # Example
///
/// ```rust,ignore
/// let controls: ClockControls = handles.get("clock.controls").unwrap();
///
/// // Adjust tempo in real-time
/// controls.set_bpm(140.0);
/// controls.set_gate_length(0.5); // 50% duty cycle
/// ```
#[derive(Clone)]
pub struct ClockControls {
    pub(crate) bpm: AtomicF32,
    pub(crate) gate_length: AtomicF32,
}

impl ClockControls {
    /// Creates new clock controls with the given initial BPM.
    ///
    /// Gate length defaults to 0.25 (25% duty cycle).
    pub fn new(bpm: f64) -> Self {
        Self {
            bpm: AtomicF32::new(bpm as f32),
            gate_length: AtomicF32::new(0.25),
        }
    }

    /// Creates new clock controls with the given BPM and gate length.
    pub fn new_with_gate_length(bpm: f64, gate_length: f64) -> Self {
        Self {
            bpm: AtomicF32::new(bpm as f32),
            gate_length: AtomicF32::new(gate_length.clamp(0.0, 1.0) as f32),
        }
    }

    /// Gets the current BPM value.
    pub fn bpm(&self) -> f64 {
        self.bpm.load() as f64
    }

    /// Gets the current BPM value.
    ///
    /// Alias for [`bpm()`](Self::bpm) for backward compatibility.
    pub fn get_bpm(&self) -> f64 {
        self.bpm()
    }

    /// Sets the tempo to a new BPM value.
    pub fn set_bpm(&self, bpm: f64) {
        self.bpm.store(bpm as f32);
    }

    /// Gets the gate length as a fraction of the pulse (0.0-1.0).
    pub fn gate_length(&self) -> f64 {
        self.gate_length.load() as f64
    }

    /// Sets the gate length as a fraction of the pulse (0.0 to 1.0).
    /// For example, 0.5 = gate HIGH for 50% of each beat.
    pub fn set_gate_length(&self, length: f64) {
        self.gate_length.store(length.clamp(0.0, 1.0) as f32);
    }

    /// Calculates the number of samples per beat at the given sample rate.
    pub fn samples_per_beat(&self, sample_rate: u32) -> f64 {
        (sample_rate as f64 * 60.0) / self.bpm()
    }
}

impl ControlSurface for ClockControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("bpm", "Tempo in beats per minute")
                .with_range(1.0, 300.0)
                .with_default(self.bpm() as f32),
            ControlMeta::number("gate_length", "Gate length as a fraction of the pulse")
                .with_range(0.0, 1.0)
                .with_default(self.gate_length() as f32),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "bpm" => Ok(ControlValue::Number(self.bpm() as f32)),
            "gate_length" => Ok(ControlValue::Number(self.gate_length() as f32)),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let value = value.as_number()?;
        match key {
            "bpm" => {
                self.set_bpm(value as f64);
                Ok(())
            }
            "gate_length" => {
                self.set_gate_length(value as f64);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}
