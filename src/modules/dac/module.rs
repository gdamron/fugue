//! DAC module for audio output.

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::modules::dac::inputs;
use crate::modules::dac::output_stage::{finite_or_silence, soft_clip};
use crate::modules::dac::outputs;
use crate::{Module, SinkModule};

/// Factory for constructing DacModule instances from configuration.
pub struct DacFactory;

impl ModuleFactory for DacFactory {
    fn type_id(&self) -> &'static str {
        "dac"
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        const { &[crate::module_config::ConfigKey::boolean("soft_clip")] }
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
/// A non-finite sample (NaN or ±inf) on either channel, after the mono
/// input is summed in, is replaced with silence (0.0) before it can reach
/// the device or a render, and counted: see
/// [`SinkModule::non_finite_samples`]. The count lives as long as the
/// module does, so it starts from zero only when the DAC is rebuilt.
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
    /// Non-finite samples replaced with silence since the module was built.
    non_finite_samples: u64,
}

impl DacModule {
    /// Creates a new DacModule with soft clipping enabled.
    pub fn new() -> Self {
        Self {
            inputs: inputs::DacInputs::new(),
            outputs: outputs::DacOutputs::new(),
            soft_clip: true,
            non_finite_samples: 0,
        }
    }

    /// Enables or disables soft clipping.
    pub fn with_soft_clip(mut self, enabled: bool) -> Self {
        self.soft_clip = enabled;
        self
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
        let mut replaced = 0;
        for i in 0..frames {
            let mut left = finite_or_silence(self.inputs.audio_left(i), &mut replaced);
            let mut right = finite_or_silence(self.inputs.audio_right(i), &mut replaced);
            if self.soft_clip {
                left = soft_clip(left);
                right = soft_clip(right);
            }
            self.outputs.set(i, left, right);
        }
        self.non_finite_samples += replaced;
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

    fn non_finite_samples(&self) -> u64 {
        self.non_finite_samples
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
    fn non_finite_samples_become_silence_and_are_counted() {
        for soft_clip in [true, false] {
            let mut dac = DacModule::new().with_soft_clip(soft_clip);
            let left = dac.input_block_mut(1);
            left[..4].copy_from_slice(&[0.25, f32::NAN, f32::INFINITY, -0.5]);
            let right = dac.input_block_mut(2);
            right[..4].copy_from_slice(&[f32::NEG_INFINITY, 0.75, 0.5, 0.125]);
            dac.process(4);
            let (l, r) = dac.sink_block();
            assert_eq!(&l[..4], &[0.25, 0.0, 0.0, -0.5]);
            assert_eq!(&r[..4], &[0.0, 0.75, 0.5, 0.125]);
            assert_eq!(dac.output_block(0)[1], 0.375, "the mono monitor is finite");
            assert_eq!(dac.non_finite_samples(), 3);

            // The count accumulates across blocks.
            dac.process(4);
            assert_eq!(dac.non_finite_samples(), 6);
        }
    }

    #[test]
    fn a_non_finite_mono_input_silences_both_channels() {
        let mut dac = DacModule::new();
        dac.set_input("audio_left", 0.5).unwrap();
        dac.set_input("audio", f32::NAN).unwrap();
        dac.process(2);
        let (l, r) = dac.sink_block();
        assert_eq!((&l[..2], &r[..2]), (&[0.0, 0.0][..], &[0.0, 0.0][..]));
        assert_eq!(dac.non_finite_samples(), 4);
    }

    #[test]
    fn finite_samples_pass_unchanged() {
        let mut dac = DacModule::new().with_soft_clip(false);
        let samples = [0.0, f32::MIN_POSITIVE, 1.5, -3.0, f32::MAX, f32::MIN];
        dac.input_block_mut(1)[..samples.len()].copy_from_slice(&samples);
        dac.process(samples.len());
        let bits = |s: &[f32]| s.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&dac.sink_block().0[..samples.len()]), bits(&samples));
        assert_eq!(dac.non_finite_samples(), 0);
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
