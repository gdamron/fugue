//! Thread-safe controls for the Clock module.

use std::sync::{Arc, Mutex};

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
/// controls.set_gate_duration(0.5); // 50% duty cycle
/// ```
#[derive(Clone)]
pub struct ClockControls {
    pub(crate) bpm: Arc<Mutex<f64>>,
    pub(crate) gate_duration: Arc<Mutex<f64>>,
}

impl ClockControls {
    /// Creates new clock controls with the given initial BPM.
    ///
    /// Gate duration defaults to 0.25 (25% duty cycle).
    pub fn new(bpm: f64) -> Self {
        Self {
            bpm: Arc::new(Mutex::new(bpm)),
            gate_duration: Arc::new(Mutex::new(0.25)),
        }
    }

    /// Creates new clock controls with the given BPM and gate duration.
    pub fn new_with_gate_duration(bpm: f64, gate_duration: f64) -> Self {
        Self {
            bpm: Arc::new(Mutex::new(bpm)),
            gate_duration: Arc::new(Mutex::new(gate_duration.clamp(0.0, 1.0))),
        }
    }

    /// Gets the current BPM value.
    pub fn bpm(&self) -> f64 {
        *self.bpm.lock().unwrap()
    }

    /// Gets the current BPM value.
    ///
    /// Alias for [`bpm()`](Self::bpm) for backward compatibility.
    pub fn get_bpm(&self) -> f64 {
        self.bpm()
    }

    /// Sets the tempo to a new BPM value.
    pub fn set_bpm(&self, bpm: f64) {
        *self.bpm.lock().unwrap() = bpm;
    }

    /// Gets the gate duration as a fraction of the beat (0.0-1.0).
    pub fn gate_duration(&self) -> f64 {
        *self.gate_duration.lock().unwrap()
    }

    /// Sets the gate duration as a fraction of the beat (0.0 to 1.0).
    /// For example, 0.5 = gate HIGH for 50% of each beat.
    pub fn set_gate_duration(&self, duration: f64) {
        *self.gate_duration.lock().unwrap() = duration.clamp(0.0, 1.0);
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
                .with_range(60.0, 300.0)
                .with_default(self.bpm() as f32),
            ControlMeta::number("gate_duration", "Gate duration as fraction of beat")
                .with_range(0.0, 1.0)
                .with_default(self.gate_duration() as f32),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "bpm" => Ok(ControlValue::Number(self.bpm() as f32)),
            "gate_duration" => Ok(ControlValue::Number(self.gate_duration() as f32)),
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
            "gate_duration" => {
                self.set_gate_duration(value as f64);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}
