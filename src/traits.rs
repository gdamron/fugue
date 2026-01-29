//! Core module traits and signal routing primitives.
//!
//! This module provides the fundamental abstraction for building synthesis graphs:
//! - [`Module`] - The unified trait for all audio processing components with named ports

/// The core abstraction for all synthesis components.
///
/// Every module in the synthesis graph implements this trait.
/// Modules process one sample at a time at audio rate, with named input and output ports.
///
/// All signals are `f32` values. The meaning of a signal is determined by which port
/// it connects to, not by its type. This design mirrors real modular synthesizers where
/// everything is voltage.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::Module;
///
/// struct Vca {
///     audio_in: f32,
///     cv_in: f32,
///     last_processed_sample: u64,
/// }
///
/// impl Module for Vca {
///     fn name(&self) -> &str {
///         "Vca"
///     }
///
///     fn process(&mut self) -> bool {
///         // Processing happens here
///         true
///     }
///
///     fn inputs(&self) -> &[&str] {
///         &["audio", "cv"]
///     }
///
///     fn outputs(&self) -> &[&str] {
///         &["audio"]
///     }
///
///     fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
///         match port {
///             "audio" => { self.audio_in = value; Ok(()) }
///             "cv" => { self.cv_in = value; Ok(()) }
///             _ => Err(format!("Unknown input port: {}", port))
///         }
///     }
///
///     fn get_output(&self, port: &str) -> Result<f32, String> {
///         match port {
///             "audio" => Ok(self.audio_in * self.cv_in),
///             _ => Err(format!("Unknown output port: {}", port))
///         }
///     }
///
///     fn last_processed_sample(&self) -> u64 {
///         self.last_processed_sample
///     }
///
///     fn mark_processed(&mut self, sample: u64) {
///         self.last_processed_sample = sample;
///     }
/// }
/// ```
pub trait Module: Send {
    /// Returns the module's name for debugging purposes.
    fn name(&self) -> &str;

    /// Processes one sample of audio.
    ///
    /// Returns `true` if the module is still active, `false` if it should be removed.
    fn process(&mut self) -> bool;

    /// Returns the names of all input ports this module accepts.
    ///
    /// Port names should be stable and descriptive (e.g., "frequency", "gate", "fm", "cv").
    fn inputs(&self) -> &[&str];

    /// Returns the names of all output ports this module provides.
    ///
    /// Port names should be stable and descriptive (e.g., "audio", "trigger", "envelope").
    fn outputs(&self) -> &[&str];

    /// Sets the value for a named input port.
    ///
    /// Called once per sample by the patch runtime for each connected input.
    /// Returns an error if the port name is not recognized.
    ///
    /// # Arguments
    ///
    /// * `port` - Name of the input port (must match one from `inputs()`)
    /// * `value` - The signal value (typically -1.0 to 1.0 for audio, 0.0 to 1.0 for CV)
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String>;

    /// Gets the current value from a named output port.
    ///
    /// Returns an error if the port name is not recognized.
    /// This method should not modify module state - it only reads cached values.
    ///
    /// # Arguments
    ///
    /// * `port` - Name of the output port (must match one from `outputs()`)
    fn get_output(&self, port: &str) -> Result<f32, String>;

    /// Returns the sample number when this module was last processed.
    ///
    /// Used by the pull-based processing system to avoid reprocessing modules
    /// multiple times within a single sample period. Modules should initialize
    /// this to 0 and update it via `mark_processed()` after each processing cycle.
    fn last_processed_sample(&self) -> u64;

    /// Marks this module as processed for the given sample number.
    ///
    /// Called by the signal graph after processing the module. This enables
    /// caching: if the same module's output is requested multiple times in
    /// one sample, it returns cached values without reprocessing.
    fn mark_processed(&mut self, sample: u64);
}

/// Helper for validating port names at module construction.
///
/// Returns `Ok(())` if the port name is in the list, `Err` otherwise.
pub fn validate_port(port: &str, valid_ports: &[&str], port_type: &str) -> Result<(), String> {
    if valid_ports.contains(&port) {
        Ok(())
    } else {
        Err(format!(
            "Unknown {} port '{}'. Valid ports: {:?}",
            port_type, port, valid_ports
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestModule {
        input_a: f32,
        input_b: f32,
        last_processed_sample: u64,
    }

    impl Module for TestModule {
        fn name(&self) -> &str {
            "TestModule"
        }

        fn process(&mut self) -> bool {
            true
        }

        fn inputs(&self) -> &[&str] {
            &["a", "b"]
        }

        fn outputs(&self) -> &[&str] {
            &["sum", "product"]
        }

        fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
            match port {
                "a" => {
                    self.input_a = value;
                    Ok(())
                }
                "b" => {
                    self.input_b = value;
                    Ok(())
                }
                _ => Err(format!("Unknown input port: {}", port)),
            }
        }

        fn get_output(&self, port: &str) -> Result<f32, String> {
            match port {
                "sum" => Ok(self.input_a + self.input_b),
                "product" => Ok(self.input_a * self.input_b),
                _ => Err(format!("Unknown output port: {}", port)),
            }
        }

        fn last_processed_sample(&self) -> u64 {
            self.last_processed_sample
        }

        fn mark_processed(&mut self, sample: u64) {
            self.last_processed_sample = sample;
        }
    }

    #[test]
    fn test_module() {
        let mut module = TestModule {
            input_a: 0.0,
            input_b: 0.0,
            last_processed_sample: 0,
        };

        // Test setting inputs
        assert!(module.set_input("a", 3.0).is_ok());
        assert!(module.set_input("b", 4.0).is_ok());
        assert!(module.set_input("c", 5.0).is_err());

        // Test getting outputs
        assert_eq!(module.get_output("sum").unwrap(), 7.0);
        assert_eq!(module.get_output("product").unwrap(), 12.0);
        assert!(module.get_output("invalid").is_err());

        // Test sample tracking
        assert_eq!(module.last_processed_sample(), 0);
        module.mark_processed(42);
        assert_eq!(module.last_processed_sample(), 42);

        // Test output after input change
        module.set_input("a", 5.0).unwrap();
        module.set_input("b", 6.0).unwrap();
        assert_eq!(module.get_output("sum").unwrap(), 11.0);
        assert_eq!(module.get_output("product").unwrap(), 30.0);
        assert!(module.get_output("invalid").is_err());
    }

    #[test]
    fn test_validate_port() {
        let ports = &["audio", "cv", "gate"];

        assert!(validate_port("audio", ports, "input").is_ok());
        assert!(validate_port("cv", ports, "input").is_ok());
        assert!(validate_port("invalid", ports, "input").is_err());
    }
}
