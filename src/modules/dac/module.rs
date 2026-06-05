//! DAC module for audio output.

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::modules::dac::inputs;
use crate::modules::dac::outputs;
use crate::{Module, SinkModule};

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

        Ok(ModuleBuildResult {
            module: GraphModule::Sink(Box::new(dac)),
            handles: vec![],
            control_surface: None,
            sink: Some(()),
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
/// - `audio`: Mono signal summed equally into left and right
/// - `audio_left`: Left channel input
/// - `audio_right`: Right channel input
///
/// # Outputs
/// - `audio`: Mono monitor output (average of left and right)
/// - `audio_left`: Left output
/// - `audio_right`: Right output
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
    outputs: outputs::DacOutputs,
    /// Whether to apply soft clipping (default: true)
    soft_clip: bool,
}

impl DacModule {
    /// Creates a new DacModule with soft clipping enabled.
    pub fn new() -> Self {
        Self {
            inputs: inputs::DacInputs::new(),
            outputs: outputs::DacOutputs::new(),
            soft_clip: true,
        }
    }

    /// Enables or disables soft clipping.
    pub fn with_soft_clip(mut self, enabled: bool) -> Self {
        self.soft_clip = enabled;
        self
    }

    /// Soft-clips audio that exceeds ±1.0 while preserving signals below the
    /// knee. Linear below `KNEE`, then smoothly compresses toward ±1.0 using
    /// `x/(1+x)` saturation on the excess. The function is C1-continuous at
    /// the knee (derivative = 1.0 from both sides), so no audible artifacts
    /// at the transition.
    #[inline]
    fn soft_clip_sample(sample: f32) -> f32 {
        const KNEE: f32 = 0.95;
        const HEADROOM: f32 = 1.0 - KNEE; // 0.05

        let abs = sample.abs();
        if abs <= KNEE {
            sample
        } else {
            let excess = (abs - KNEE) / HEADROOM;
            let compressed = KNEE + HEADROOM * excess / (1.0 + excess);
            sample.signum() * compressed
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

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let (mut left, mut right) = (self.inputs.audio_left(i), self.inputs.audio_right(i));
            if self.soft_clip {
                left = Self::soft_clip_sample(left);
                right = Self::soft_clip_sample(right);
            }
            self.outputs.set(i, left, right);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }
}

impl SinkModule for DacModule {
    fn sink_block(&self) -> (&[f32], &[f32]) {
        (self.outputs.left_block(), self.outputs.right_block())
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
        dac.process(1);

        // Below knee (0.95), soft clip is linear pass-through
        let out = dac.get_output("audio").unwrap();
        assert_eq!(
            out, 0.5,
            "Below knee, should be exact pass-through, got {}",
            out
        );
        assert_eq!(dac.sink_block().0[0], out);
        assert_eq!(dac.sink_block().1[0], out);
    }

    #[test]
    fn test_dac_module_without_soft_clip() {
        let mut dac = DacModule::new().with_soft_clip(false);

        dac.set_input("audio_left", 0.25).unwrap();
        dac.set_input("audio_right", 0.75).unwrap();
        dac.process(1);

        assert_eq!(dac.get_output("audio_left").unwrap(), 0.25);
        assert_eq!(dac.get_output("audio_right").unwrap(), 0.75);
    }

    #[test]
    fn test_dac_soft_clip_prevents_clipping() {
        let mut dac = DacModule::new();

        // Very hot signal that would normally clip
        dac.set_input("audio", 3.0).unwrap();
        dac.process(1);

        let out = dac.get_output("audio").unwrap();
        // Should be limited below 1.0
        assert!(out < 1.0, "Soft clip should limit output, got {}", out);
        assert!(out > 0.99, "Should still be close to 1.0, got {}", out);
    }

    #[test]
    fn test_dac_soft_clip_symmetrical() {
        let mut dac = DacModule::new();

        // Test positive
        dac.set_input("audio", 2.0).unwrap();
        dac.process(1);
        let pos = dac.get_output("audio").unwrap();

        // Test negative
        dac.set_input("audio", -2.0).unwrap();
        dac.process(1);
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
        let mut result = factory
            .build(44100, &serde_json::json!({"soft_clip": false}))
            .expect("Failed to build DacModule");

        let module = result.module.module_mut();
        module.set_input("audio", 0.5).ok();
        // Can't easily test the soft_clip field, but we can verify it builds
    }
}
