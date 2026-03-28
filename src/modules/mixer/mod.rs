//! Mixer module for combining multiple audio signals.
//!
//! The mixer combines multiple input signals with individual level controls
//! and a master output level. This is essential for layering sounds, creating
//! complex timbres, and balancing multiple voices.
//!
//! # Features
//!
//! - 4 input channels (default, configurable 1-8)
//! - Per-channel level control (0.0 to 1.0)
//! - Per-channel equal-power panning (-1.0 left to 1.0 right)
//! - Master output level
//! - CV inputs for dynamic level and pan control
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc1", "type": "oscillator", "config": { "oscillator_type": "sawtooth" } },
//!     { "id": "osc2", "type": "oscillator", "config": { "oscillator_type": "square" } },
//!     {
//!       "id": "mixer",
//!       "type": "mixer",
//!       "config": { "channels": 2, "levels": [0.5, 0.3], "pans": [-0.3, 0.3] }
//!     },
//!     { "id": "dac", "type": "dac" }
//!   ],
//!   "connections": [
//!     { "from": "osc1", "from_port": "audio", "to": "mixer", "to_port": "in1" },
//!     { "from": "osc2", "from_port": "audio", "to": "mixer", "to_port": "in2" },
//!     { "from": "mixer", "from_port": "left", "to": "dac", "to_port": "audio_left" },
//!     { "from": "mixer", "from_port": "right", "to": "dac", "to_port": "audio_right" }
//!   ]
//! }
//! ```

use std::any::Any;
use std::f32::consts::FRAC_PI_4;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::MixerControls;

mod controls;
mod inputs;
mod outputs;

/// Maximum number of mixer channels supported.
pub const MAX_CHANNELS: usize = 8;

/// A multi-channel audio mixer.
///
/// Combines multiple audio inputs with individual level controls. Each channel
/// has both a base level (set via configuration) and a CV input for dynamic
/// control, plus a pan control with optional modulation. The final output is a
/// stereo pair summed across all channels and multiplied by the master level.
///
/// # Inputs
///
/// - `in1` through `in8` - Audio inputs (depending on channel count)
/// - `level1` through `level8` - Level CV inputs (multiplied with base level)
/// - `pan1` through `pan8` - Pan modulation inputs (added to base pan)
/// - `master` - Master output level CV
///
/// # Outputs
///
/// - `left` - Mixed left-channel output
/// - `right` - Mixed right-channel output
///
/// # Controls
///
/// - `level.0` through `level.7` - Per-channel base levels
/// - `pan.0` through `pan.7` - Per-channel base pan positions
/// - `master` - Master output level
///
/// # Example
///
/// ```rust,ignore
/// use fugue::modules::mixer::Mixer;
///
/// let mut mixer = Mixer::new(4)
///     .with_level(0, 0.8)   // Channel 1 at 80%
///     .with_level(1, 0.5)   // Channel 2 at 50%
///     .with_pan(0, -0.3)    // Channel 1 left
///     .with_pan(1, 0.3)     // Channel 2 right
///     .with_master(0.7);    // Master at 70%
/// ```
pub struct Mixer {
    channels: usize,
    ctrl: MixerControls,
    input_state: inputs::MixerInputs,

    // Pull-based processing
    last_processed_sample: u64,
}

impl Mixer {
    /// Creates a new mixer with the specified number of channels.
    ///
    /// Channel count is clamped to 1-8. Default levels are 1.0 (unity gain).
    pub fn new(channels: usize) -> Self {
        let channels = channels.clamp(1, MAX_CHANNELS);
        let controls = MixerControls::new(channels);
        Self::new_with_controls(channels, controls)
    }

    /// Creates a new mixer with the given controls.
    pub fn new_with_controls(channels: usize, controls: MixerControls) -> Self {
        let channels = channels.clamp(1, MAX_CHANNELS);

        Self {
            channels,
            ctrl: controls,
            input_state: inputs::MixerInputs::new(channels),
            last_processed_sample: 0,
        }
    }

    /// Sets the level for a specific channel (0-indexed).
    pub fn with_level(self, channel: usize, level: f32) -> Self {
        self.ctrl.set_level(channel, level);
        self
    }

    /// Sets the pan for a specific channel (0-indexed).
    pub fn with_pan(self, channel: usize, pan: f32) -> Self {
        self.ctrl.set_pan(channel, pan);
        self
    }

    /// Sets the master output level.
    pub fn with_master(self, level: f32) -> Self {
        self.ctrl.set_master(level);
        self
    }

    /// Sets the level for a specific channel.
    pub fn set_level(&mut self, channel: usize, level: f32) {
        self.ctrl.set_level(channel, level);
    }

    /// Sets the pan for a specific channel.
    pub fn set_pan(&mut self, channel: usize, pan: f32) {
        self.ctrl.set_pan(channel, pan);
    }

    /// Sets the master output level.
    pub fn set_master(&mut self, level: f32) {
        self.ctrl.set_master(level);
    }

    /// Returns the number of channels.
    pub fn channel_count(&self) -> usize {
        self.channels
    }

    /// Returns a reference to the controls.
    pub fn controls(&self) -> &MixerControls {
        &self.ctrl
    }

    fn effective_pan(&self, channel: usize) -> f32 {
        (self.ctrl.pan(channel) + self.input_state.pan_mod(channel)).clamp(-1.0, 1.0)
    }

    /// Mixes all inputs and returns the stereo output sample.
    fn mix(&self) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;
        let levels = self.ctrl.levels.lock().unwrap();

        for i in 0..self.channels {
            let level_cv = self.input_state.level_cv(i);
            let channel_out = self.input_state.audio(i) * levels[i] * level_cv;
            let pan = self.effective_pan(i);
            let angle = (pan + 1.0) * FRAC_PI_4;
            left += channel_out * angle.cos();
            right += channel_out * angle.sin();
        }

        let master_level = self.ctrl.master();
        let master_cv = self.input_state.master_cv();
        let gain = master_level * master_cv;

        (left * gain, right * gain)
    }
}

impl Module for Mixer {
    fn name(&self) -> &str {
        "Mixer"
    }

    fn process(&mut self) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        self.input_state.names()
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.input_state.set(self.channels, port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let (left, right) = self.mix();
        outputs::MixerOutputs::get(port, left, right)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn reset_inputs(&mut self) {
        self.input_state.reset();
    }

    fn controls(&self) -> Vec<ControlMeta> {
        let mut controls = Vec::with_capacity(self.channels * 2 + 1);

        for i in 0..self.channels {
            controls.push(
                ControlMeta::new(format!("level.{}", i), format!("Channel {} level", i + 1))
                    .with_range(0.0, 2.0)
                    .with_default(1.0),
            );
            controls.push(
                ControlMeta::new(format!("pan.{}", i), format!("Channel {} pan", i + 1))
                    .with_range(-1.0, 1.0)
                    .with_default(0.0),
            );
        }

        controls.push(
            ControlMeta::new("master", "Master output level")
                .with_range(0.0, 2.0)
                .with_default(1.0),
        );

        controls
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        if key == "master" {
            return Ok(self.ctrl.master());
        }

        if let Some(rest) = key.strip_prefix("level.") {
            if let Ok(idx) = rest.parse::<usize>() {
                if idx < self.channels {
                    return Ok(self.ctrl.level(idx));
                }
            }
        }

        if let Some(rest) = key.strip_prefix("pan.") {
            if let Ok(idx) = rest.parse::<usize>() {
                if idx < self.channels {
                    return Ok(self.ctrl.pan(idx));
                }
            }
        }

        Err(format!("Unknown control: {}", key))
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        if key == "master" {
            self.ctrl.set_master(value);
            return Ok(());
        }

        if let Some(rest) = key.strip_prefix("level.") {
            if let Ok(idx) = rest.parse::<usize>() {
                if idx < self.channels {
                    self.ctrl.set_level(idx, value);
                    return Ok(());
                }
            }
        }

        if let Some(rest) = key.strip_prefix("pan.") {
            if let Ok(idx) = rest.parse::<usize>() {
                if idx < self.channels {
                    self.ctrl.set_pan(idx, value);
                    return Ok(());
                }
            }
        }

        Err(format!("Unknown control: {}", key))
    }
}

/// Factory for constructing Mixer modules from configuration.
///
/// # Configuration Options
///
/// - `channels` (usize): Number of input channels, 1-8 (default: 4)
/// - `levels` (array of f32): Initial level for each channel (default: all 1.0)
/// - `pans` (array of f32): Initial pan position for each channel (-1.0 to 1.0, default: all 0.0)
/// - `master` (f32): Master output level (default: 1.0)
///
/// # Example
///
/// ```json
/// {
///   "id": "main_mixer",
///   "type": "mixer",
///   "config": {
///     "channels": 4,
///     "levels": [0.8, 0.6, 0.4, 0.3],
///     "pans": [-0.5, -0.15, 0.15, 0.5],
///     "master": 0.8
///   }
/// }
/// ```
pub struct MixerFactory;

impl ModuleFactory for MixerFactory {
    fn type_id(&self) -> &'static str {
        "mixer"
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let channels = config
            .get("channels")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(4);

        let master = config.get("master").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;

        let levels: Vec<f32> = config
            .get("levels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|n| n as f32))
                    .collect()
            })
            .unwrap_or_else(Vec::new);

        let pans: Vec<f32> = config
            .get("pans")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|n| n as f32))
                    .collect()
            })
            .unwrap_or_else(Vec::new);

        let controls = MixerControls::new_with_config(channels, &levels, &pans, master);
        let mixer = Mixer::new_with_controls(channels, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(mixer)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.001,
            "Expected {}, got {}",
            expected,
            actual
        );
    }

    #[test]
    fn test_mixer_basic_summing() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 0.5).unwrap();
        mixer.set_input("in2", 0.3).unwrap();
        mixer.process();

        let left = mixer.get_output("left").unwrap();
        let right = mixer.get_output("right").unwrap();

        approx_eq(left, 0.8 * 2.0_f32.sqrt().recip());
        approx_eq(right, 0.8 * 2.0_f32.sqrt().recip());
    }

    #[test]
    fn test_mixer_with_levels() {
        let mut mixer = Mixer::new(2)
            .with_level(0, 0.5) // Channel 1 at 50%
            .with_level(1, 1.0); // Channel 2 at 100%

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.process();

        let left = mixer.get_output("left").unwrap();
        let right = mixer.get_output("right").unwrap();
        let expected = 1.5 * 2.0_f32.sqrt().recip();

        approx_eq(left, expected);
        approx_eq(right, expected);
    }

    #[test]
    fn test_mixer_master_level() {
        let mut mixer = Mixer::new(2).with_master(0.5);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.process();

        let left = mixer.get_output("left").unwrap();
        let right = mixer.get_output("right").unwrap();
        let expected = 2.0_f32.sqrt().recip();

        approx_eq(left, expected);
        approx_eq(right, expected);
    }

    #[test]
    fn test_mixer_level_cv() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.set_input("level1", 0.5).unwrap(); // CV reduces channel 1
        mixer.set_input("level2", 1.0).unwrap();
        mixer.process();

        let left = mixer.get_output("left").unwrap();
        let right = mixer.get_output("right").unwrap();
        let expected = 1.5 * 2.0_f32.sqrt().recip();

        approx_eq(left, expected);
        approx_eq(right, expected);
    }

    #[test]
    fn test_mixer_master_cv() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.set_input("master", 0.25).unwrap();
        mixer.process();

        let left = mixer.get_output("left").unwrap();
        let right = mixer.get_output("right").unwrap();
        let expected = 0.5 * 2.0_f32.sqrt().recip();

        approx_eq(left, expected);
        approx_eq(right, expected);
    }

    #[test]
    fn test_mixer_channel_count() {
        let mixer = Mixer::new(3);
        assert_eq!(mixer.channel_count(), 3);

        // Check that only 3 input channels exist
        let inputs = mixer.inputs();
        assert!(inputs.contains(&"in1"));
        assert!(inputs.contains(&"in2"));
        assert!(inputs.contains(&"in3"));
        assert!(!inputs.contains(&"in4"));

        assert!(inputs.contains(&"level1"));
        assert!(inputs.contains(&"level2"));
        assert!(inputs.contains(&"level3"));
        assert!(!inputs.contains(&"level4"));

        assert!(inputs.contains(&"pan1"));
        assert!(inputs.contains(&"pan2"));
        assert!(inputs.contains(&"pan3"));
        assert!(!inputs.contains(&"pan4"));
    }

    #[test]
    fn test_mixer_invalid_port() {
        let mut mixer = Mixer::new(2);

        // in3 doesn't exist on 2-channel mixer
        assert!(mixer.set_input("in3", 1.0).is_err());
        assert!(mixer.set_input("level3", 1.0).is_err());
        assert!(mixer.set_input("pan3", 1.0).is_err());
        assert!(mixer.get_output("invalid").is_err());
    }

    #[test]
    fn test_mixer_factory() {
        let factory = MixerFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "mixer");

        let config = serde_json::json!({
            "channels": 3,
            "levels": [0.8, 0.6, 0.4],
            "pans": [-1.0, 0.0, 1.0],
            "master": 0.9
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Mixer");

        // Should have in1-in3, level1-level3, pan1-pan3, and master
        let inputs = module.inputs();
        assert_eq!(inputs.len(), 10); // 3 ins + 3 levels + 3 pans + 1 master
    }

    #[test]
    fn test_mixer_clamps_channels() {
        // Too few
        let mixer = Mixer::new(0);
        assert_eq!(mixer.channel_count(), 1);

        // Too many
        let mixer = Mixer::new(100);
        assert_eq!(mixer.channel_count(), MAX_CHANNELS);
    }

    #[test]
    fn test_mixer_negative_input() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 0.5).unwrap();
        mixer.set_input("in2", -0.5).unwrap();
        mixer.process();

        let left = mixer.get_output("left").unwrap();
        let right = mixer.get_output("right").unwrap();
        assert!(left.abs() < 0.001, "Expected ~0 left, got {}", left);
        assert!(right.abs() < 0.001, "Expected ~0 right, got {}", right);
    }

    #[test]
    fn test_mixer_hard_panning() {
        let mut mixer = Mixer::new(2).with_pan(0, -1.0).with_pan(1, 1.0);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.process();

        approx_eq(mixer.get_output("left").unwrap(), 1.0);
        approx_eq(mixer.get_output("right").unwrap(), 1.0);
    }

    #[test]
    fn test_mixer_pan_modulation_input() {
        let mut mixer = Mixer::new(1);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("pan1", 1.0).unwrap();
        mixer.process();

        approx_eq(mixer.get_output("left").unwrap(), 0.0);
        approx_eq(mixer.get_output("right").unwrap(), 1.0);
    }

    #[test]
    fn test_mixer_controls() {
        let mut mixer = Mixer::new(2);

        // Test control metadata
        let control_meta = Module::controls(&mixer);
        assert_eq!(control_meta.len(), 5); // level.0, pan.0, level.1, pan.1, master
        assert_eq!(control_meta[0].key, "level.0");
        assert_eq!(control_meta[1].key, "pan.0");
        assert_eq!(control_meta[2].key, "level.1");
        assert_eq!(control_meta[3].key, "pan.1");
        assert_eq!(control_meta[4].key, "master");

        // Test get/set controls
        mixer.set_control("level.0", 0.5).unwrap();
        assert_eq!(mixer.get_control("level.0").unwrap(), 0.5);

        mixer.set_control("pan.0", -0.25).unwrap();
        assert_eq!(mixer.get_control("pan.0").unwrap(), -0.25);

        mixer.set_control("master", 0.8).unwrap();
        assert_eq!(mixer.get_control("master").unwrap(), 0.8);

        // Test invalid control
        assert!(mixer.get_control("invalid").is_err());
        assert!(mixer.get_control("level.5").is_err()); // Only 2 channels
        assert!(mixer.get_control("pan.5").is_err()); // Only 2 channels
    }
}
