//! DAC module for audio output.

use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::{Module, SinkModule, SinkOutput};

/// Factory for constructing DacModule instances from configuration.
pub struct DacFactory;

impl ModuleFactory for DacFactory {
    fn type_id(&self) -> &'static str {
        "dac"
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let dac = DacModule::new();
        let dac_arc = Arc::new(Mutex::new(dac));

        Ok(ModuleBuildResult {
            module: dac_arc.clone(),
            handles: vec![],
            sink: Some(dac_arc),
        })
    }

    fn is_sink(&self) -> bool {
        true
    }
}

/// DAC (Digital-to-Analog Converter) module.
///
/// Acts as a sink that collects audio for output to the audio backend.
/// Implements both `Module` (for signal graph integration) and `SinkModule`
/// (for audio output collection).
///
/// # Inputs
/// - `audio`: The audio signal to output (typically -1.0 to 1.0)
///
/// # Outputs
/// - `audio`: Pass-through of the input (for potential downstream chaining)
///
/// # Example
///
/// ```json
/// {
///   "id": "dac",
///   "type": "dac"
/// }
/// ```
pub struct DacModule {
    /// Input audio sample
    audio_in: f32,
    /// Output audio sample (for pass-through)
    audio_out: f32,
    /// Last processed sample number (for caching)
    last_processed_sample: u64,
}

impl DacModule {
    /// Creates a new DacModule.
    pub fn new() -> Self {
        Self {
            audio_in: 0.0,
            audio_out: 0.0,
            last_processed_sample: 0,
        }
    }
}

impl Default for DacModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for DacModule {
    fn name(&self) -> &str {
        "DacModule"
    }

    fn process(&mut self) -> bool {
        // Pass through the audio
        self.audio_out = self.audio_in;
        true
    }

    fn inputs(&self) -> &[&str] {
        &["audio"]
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
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio_out),
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

impl SinkModule for DacModule {
    fn sink_output(&self) -> SinkOutput {
        SinkOutput::mono(self.audio_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dac_module_basic() {
        let mut dac = DacModule::new();

        // Initial state
        assert_eq!(dac.get_output("audio").unwrap(), 0.0);

        // Set input and process
        dac.set_input("audio", 0.5).unwrap();
        dac.process();

        // Check output
        assert_eq!(dac.get_output("audio").unwrap(), 0.5);
        assert_eq!(dac.sink_output().sample, 0.5);
    }

    #[test]
    fn test_dac_module_invalid_port() {
        let mut dac = DacModule::new();

        assert!(dac.set_input("invalid", 0.5).is_err());
        assert!(dac.get_output("invalid").is_err());
    }

    #[test]
    fn test_dac_module_sample_tracking() {
        let mut dac = DacModule::new();

        assert_eq!(dac.last_processed_sample(), 0);
        dac.mark_processed(42);
        assert_eq!(dac.last_processed_sample(), 42);
    }

    #[test]
    fn test_dac_factory() {
        let factory = DacFactory;

        assert_eq!(factory.type_id(), "dac");
        assert!(factory.is_sink());

        let result = factory
            .build(44100, &serde_json::json!({}))
            .expect("Failed to build DacModule");

        assert!(result.sink.is_some());
    }
}
