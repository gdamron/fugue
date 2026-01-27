//! Named port module system for flexible signal routing.
//!
//! This module provides an alternative to the type-based `Generator`/`Processor` system.
//! Instead of using Rust's type system to enforce signal compatibility, modules declare
//! named input and output ports, and all signals are uniform `f32` values.
//!
//! This approach mirrors real modular synthesizers where all signals are voltages,
//! and the meaning of a signal is determined by which port it's connected to.
//!
//! # Example
//!
//! ```rust,ignore
//! use fugue::module::ModularModule;
//!
//! struct VCA {
//!     audio_in: f32,
//!     cv_in: f32,
//! }
//!
//! impl ModularModule for VCA {
//!     fn inputs(&self) -> &[&str] {
//!         &["audio", "cv"]
//!     }
//!
//!     fn outputs(&self) -> &[&str] {
//!         &["audio"]
//!     }
//!
//!     fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
//!         match port {
//!             "audio" => self.audio_in = value,
//!             "cv" => self.cv_in = value,
//!             _ => return Err(format!("Unknown input port: {}", port)),
//!         }
//!         Ok(())
//!     }
//!
//!     fn get_output(&mut self, port: &str) -> Result<f32, String> {
//!         match port {
//!             "audio" => Ok(self.audio_in * self.cv_in),
//!             _ => Err(format!("Unknown output port: {}", port)),
//!         }
//!     }
//! }
//! ```

use super::Module;

/// A module with named input and output ports for flexible signal routing.
///
/// Unlike the type-based `Generator`/`Processor` traits, `ModularModule` treats
/// all signals as uniform `f32` values. The meaning of a signal is determined
/// by which port it connects to, not by its Rust type.
///
/// This design mirrors real modular synthesizers where everything is voltage.
pub trait ModularModule: Module {
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
    /// Called once per sample by the patch runtime for each connected output.
    /// Returns an error if the port name is not recognized.
    ///
    /// # Arguments
    ///
    /// * `port` - Name of the output port (must match one from `outputs()`)
    fn get_output(&mut self, port: &str) -> Result<f32, String>;

    /// Resets all inputs to their default values.
    ///
    /// Called at the start of each sample to clear any unconnected inputs.
    /// Default implementation does nothing, but modules should override this
    /// if they need to handle unconnected inputs specially (e.g., default to 0.0).
    ///
    /// **Note:** This method is deprecated in the pull-based architecture and may
    /// be removed in future versions. Modules should set sensible defaults in their
    /// constructors instead.
    fn reset_inputs(&mut self) {}

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

    /// Gets cached output value without triggering processing.
    ///
    /// Returns the output value computed during the last `process()` call.
    /// This method should never modify module state or trigger side effects.
    ///
    /// # Arguments
    ///
    /// * `port` - Name of the output port (must match one from `outputs()`)
    ///
    /// # Returns
    ///
    /// The cached output value, or an error if the port name is invalid.
    fn get_cached_output(&self, port: &str) -> Result<f32, String>;
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
    }

    impl ModularModule for TestModule {
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

        fn get_output(&mut self, port: &str) -> Result<f32, String> {
            match port {
                "sum" => Ok(self.input_a + self.input_b),
                "product" => Ok(self.input_a * self.input_b),
                _ => Err(format!("Unknown output port: {}", port)),
            }
        }

        fn reset_inputs(&mut self) {
            self.input_a = 0.0;
            self.input_b = 0.0;
        }

        fn last_processed_sample(&self) -> u64 {
            self.last_processed_sample
        }

        fn mark_processed(&mut self, sample: u64) {
            self.last_processed_sample = sample;
        }

        fn get_cached_output(&self, port: &str) -> Result<f32, String> {
            match port {
                "sum" => Ok(self.input_a + self.input_b),
                "product" => Ok(self.input_a * self.input_b),
                _ => Err(format!("Unknown output port: {}", port)),
            }
        }
    }

    #[test]
    fn test_modular_module() {
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

        // Test reset
        module.reset_inputs();
        assert_eq!(module.get_output("sum").unwrap(), 0.0);

        // Test sample tracking
        assert_eq!(module.last_processed_sample(), 0);
        module.mark_processed(42);
        assert_eq!(module.last_processed_sample(), 42);

        // Test cached output
        module.set_input("a", 5.0).unwrap();
        module.set_input("b", 6.0).unwrap();
        assert_eq!(module.get_cached_output("sum").unwrap(), 11.0);
        assert_eq!(module.get_cached_output("product").unwrap(), 30.0);
        assert!(module.get_cached_output("invalid").is_err());
    }

    #[test]
    fn test_validate_port() {
        let ports = &["audio", "cv", "gate"];

        assert!(validate_port("audio", ports, "input").is_ok());
        assert!(validate_port("cv", ports, "input").is_ok());
        assert!(validate_port("invalid", ports, "input").is_err());
    }
}
