//! Voltage Controlled Amplifier (VCA) module.
//!
//! A VCA multiplies an audio signal by a control voltage, allowing dynamic
//! amplitude control. Common uses include applying envelope shapes to sounds,
//! tremolo effects, and level control.

use crate::{ModularModule, Module};

/// A Voltage Controlled Amplifier that multiplies audio by a control voltage.
///
/// # Inputs
/// - `audio`: The audio signal to be amplified (typically -1.0 to 1.0)
/// - `cv`: Control voltage for amplitude (0.0 to 1.0, where 1.0 = full volume)
///
/// # Outputs
/// - `audio`: The amplified audio signal (audio * cv)
///
/// # Example
///
/// ```rust,ignore
/// // Connect an envelope to control a VCA
/// // In patch JSON:
/// {
///   "connections": [
///     {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"},
///     {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"},
///     {"from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio"}
///   ]
/// }
/// ```
pub struct Vca {
    audio_in: f32,
    cv_in: f32,
    last_processed_sample: u64, // For pull-based processing
}

impl Vca {
    /// Creates a new VCA with CV defaulting to 1.0 (unity gain/passthrough).
    pub fn new() -> Self {
        Self {
            audio_in: 0.0,
            cv_in: 1.0,
            last_processed_sample: 0,
        }
    }
}

impl Default for Vca {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Vca {
    fn name(&self) -> &str {
        "Vca"
    }
}

impl ModularModule for Vca {
    fn inputs(&self) -> &[&str] {
        &["audio", "cv"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio_in = value;
                Ok(())
            }
            "cv" => {
                self.cv_in = value.clamp(0.0, 1.0);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&mut self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio_in * self.cv_in),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn reset_inputs(&mut self) {
        self.audio_in = 0.0;
        // Don't reset cv_in - it should stay at its last value (or default 1.0)
        // This allows VCA to act as passthrough when no CV is connected
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn get_cached_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio_in * self.cv_in),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vca_basic() {
        let mut vca = Vca::new();

        // Full volume
        vca.set_input("audio", 0.5).unwrap();
        vca.set_input("cv", 1.0).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.5);

        // Half volume
        vca.set_input("cv", 0.5).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.25);

        // Silence
        vca.set_input("cv", 0.0).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.0);
    }

    #[test]
    fn test_vca_cv_clamping() {
        let mut vca = Vca::new();

        vca.set_input("audio", 1.0).unwrap();

        // CV above 1.0 should be clamped
        vca.set_input("cv", 2.0).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 1.0);

        // CV below 0.0 should be clamped
        vca.set_input("cv", -0.5).unwrap();
        assert_eq!(vca.get_output("audio").unwrap(), 0.0);
    }

    #[test]
    fn test_vca_invalid_ports() {
        let mut vca = Vca::new();

        assert!(vca.set_input("invalid", 0.5).is_err());
        assert!(vca.get_output("invalid").is_err());
    }

    #[test]
    fn test_vca_reset() {
        let mut vca = Vca::new();

        vca.set_input("audio", 0.8).unwrap();
        vca.set_input("cv", 0.6).unwrap();

        vca.reset_inputs();

        assert_eq!(vca.get_output("audio").unwrap(), 0.0);
    }
}
