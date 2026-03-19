//! DAC module for audio output.

use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::modules::dac::inputs;
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
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let mut dac = DacModule::new();

        // Optional: disable soft clipping (default is enabled)
        if let Some(soft_clip) = config.get("soft_clip").and_then(|v| v.as_bool()) {
            dac.soft_clip = soft_clip;
        }

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
/// By default, applies soft clipping (tanh-based) to prevent harsh digital
/// clipping when signals exceed the -1.0 to 1.0 range.
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
///   "type": "dac",
///   "config": {
///     "soft_clip": true
///   }
/// }
/// ```
pub struct DacModule {
    /// Input audio sample
    inputs: inputs::DacInputs,
    /// Output audio sample (after processing)
    audio_out: f32,
    /// Whether to apply soft clipping (default: true)
    soft_clip: bool,
    /// Last processed sample number (for caching)
    last_processed_sample: u64,
}

impl DacModule {
    /// Creates a new DacModule with soft clipping enabled.
    pub fn new() -> Self {
        Self {
            inputs: inputs::DacInputs::new(),
            audio_out: 0.0,
            soft_clip: true,
            last_processed_sample: 0,
        }
    }

    /// Enables or disables soft clipping.
    pub fn with_soft_clip(mut self, enabled: bool) -> Self {
        self.soft_clip = enabled;
        self
    }

    /// Applies soft clipping using tanh-based saturation.
    ///
    /// This provides a gentle rolloff as signals approach and exceed ±1.0,
    /// which sounds more musical than hard clipping.
    #[inline]
    fn soft_clip_sample(sample: f32) -> f32 {
        // tanh provides smooth saturation
        // Scale input slightly to make the knee less aggressive
        sample.tanh()
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
        self.audio_out = if self.soft_clip {
            Self::soft_clip_sample(self.inputs.audio())
        } else {
            self.inputs.audio()
        };
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn reset_inputs(&mut self) {
        self.inputs.reset();
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
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

        // Check output (soft clipping affects the value slightly)
        let out = dac.get_output("audio").unwrap();
        assert!((out - 0.4621).abs() < 0.01, "Expected ~0.462, got {}", out);
        assert_eq!(dac.sink_output().sample, out);
    }

    #[test]
    fn test_dac_module_without_soft_clip() {
        let mut dac = DacModule::new().with_soft_clip(false);

        dac.set_input("audio", 0.5).unwrap();
        dac.process();

        // Without soft clip, should be exact pass-through
        assert_eq!(dac.get_output("audio").unwrap(), 0.5);
    }

    #[test]
    fn test_dac_soft_clip_prevents_clipping() {
        let mut dac = DacModule::new();

        // Very hot signal that would normally clip
        dac.set_input("audio", 3.0).unwrap();
        dac.process();

        let out = dac.get_output("audio").unwrap();
        // Should be limited below 1.0
        assert!(out < 1.0, "Soft clip should limit output, got {}", out);
        assert!(out > 0.99, "Should still be close to 1.0, got {}", out);
    }

    #[test]
    fn test_dac_soft_clip_symmetrical() {
        let mut dac = DacModule::new();

        // Test positive
        dac.reset_inputs();
        dac.set_input("audio", 2.0).unwrap();
        dac.process();
        let pos = dac.get_output("audio").unwrap();

        // Test negative
        dac.reset_inputs();
        dac.set_input("audio", -2.0).unwrap();
        dac.process();
        let neg = dac.get_output("audio").unwrap();

        assert!((pos + neg).abs() < 0.001, "Soft clip should be symmetrical");
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

    #[test]
    fn test_dac_factory_soft_clip_config() {
        let factory = DacFactory;

        // Test with soft_clip disabled
        let result = factory
            .build(44100, &serde_json::json!({"soft_clip": false}))
            .expect("Failed to build DacModule");

        let mut module = result.module.lock().unwrap();
        module.set_input("audio", 0.5).ok();
        // Can't easily test the soft_clip field, but we can verify it builds
    }
}
