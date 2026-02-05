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
//! - Master output level
//! - CV inputs for dynamic level control
//!
//! # Example Patch
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc1", "type": "oscillator", "config": { "oscillator_type": "sawtooth" } },
//!     { "id": "osc2", "type": "oscillator", "config": { "oscillator_type": "square" } },
//!     { "id": "mixer", "type": "mixer", "config": { "channels": 2, "levels": [0.5, 0.3] } }
//!   ],
//!   "connections": [
//!     { "from": "osc1", "from_port": "audio", "to": "mixer", "to_port": "in1" },
//!     { "from": "osc2", "from_port": "audio", "to": "mixer", "to_port": "in2" }
//!   ]
//! }
//! ```

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::MixerControls;

mod controls;

/// Maximum number of mixer channels supported.
pub const MAX_CHANNELS: usize = 8;

/// A multi-channel audio mixer.
///
/// Combines multiple audio inputs with individual level controls. Each channel
/// has both a base level (set via configuration) and a CV input for dynamic
/// control. The final output is the sum of all channels multiplied by the
/// master level.
///
/// # Inputs
///
/// - `in1` through `in8` - Audio inputs (depending on channel count)
/// - `level1` through `level8` - Level CV inputs (multiplied with base level)
/// - `master` - Master output level CV
///
/// # Outputs
///
/// - `out` - Mixed audio output
///
/// # Controls
///
/// - `level.0` through `level.7` - Per-channel base levels
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
///     .with_master(0.7);    // Master at 70%
/// ```
pub struct Mixer {
    channels: usize,
    ctrl: MixerControls,

    // Input values
    inputs: [f32; MAX_CHANNELS],
    level_cvs: [f32; MAX_CHANNELS],
    master_cv: f32,

    // Active flags for signal override
    level_cv_active: [bool; MAX_CHANNELS],
    master_cv_active: bool,

    // For dynamic port lists
    input_names: Vec<&'static str>,
    output_names: Vec<&'static str>,

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

        // Build input port names based on channel count
        let mut input_names = Vec::with_capacity(channels * 2 + 1);
        for name in INPUT_NAMES.iter().take(channels) {
            input_names.push(*name);
        }
        for name in LEVEL_NAMES.iter().take(channels) {
            input_names.push(*name);
        }
        input_names.push("master");

        Self {
            channels,
            ctrl: controls,
            inputs: [0.0; MAX_CHANNELS],
            level_cvs: [1.0; MAX_CHANNELS], // Default CV is unity (no change)
            master_cv: 1.0,
            level_cv_active: [false; MAX_CHANNELS],
            master_cv_active: false,
            input_names,
            output_names: vec!["out"],
            last_processed_sample: 0,
        }
    }

    /// Sets the level for a specific channel (0-indexed).
    pub fn with_level(self, channel: usize, level: f32) -> Self {
        self.ctrl.set_level(channel, level);
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

    /// Mixes all inputs and returns the output sample.
    fn mix(&self) -> f32 {
        let mut sum = 0.0;
        let levels = self.ctrl.levels.lock().unwrap();

        for i in 0..self.channels {
            // Use CV if active, otherwise use control level
            let level_cv = if self.level_cv_active[i] {
                self.level_cvs[i]
            } else {
                1.0 // Unity when no CV connected
            };
            // Channel output = input * base_level * level_cv
            let channel_out = self.inputs[i] * levels[i] * level_cv;
            sum += channel_out;
        }

        // Use master CV if active, otherwise use control master
        let master_level = self.ctrl.master();
        let master_cv = if self.master_cv_active {
            self.master_cv
        } else {
            1.0 // Unity when no CV connected
        };

        // Apply master level
        sum * master_level * master_cv
    }
}

// Static port name arrays for lifetime management
static INPUT_NAMES: [&str; MAX_CHANNELS] = ["in1", "in2", "in3", "in4", "in5", "in6", "in7", "in8"];
static LEVEL_NAMES: [&str; MAX_CHANNELS] = [
    "level1", "level2", "level3", "level4", "level5", "level6", "level7", "level8",
];

impl Module for Mixer {
    fn name(&self) -> &str {
        "Mixer"
    }

    fn process(&mut self) -> bool {
        // Output is computed fresh in get_output()
        // No caching needed since mix() is stateless
        true
    }

    fn inputs(&self) -> &[&str] {
        &self.input_names
    }

    fn outputs(&self) -> &[&str] {
        &self.output_names
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        // Check for audio inputs (in1-in8)
        if let Some(rest) = port.strip_prefix("in") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1; // Convert 1-indexed to 0-indexed
                if idx < self.channels {
                    self.inputs[idx] = value;
                    return Ok(());
                }
            }
        }

        // Check for level CVs (level1-level8)
        if let Some(rest) = port.strip_prefix("level") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < self.channels {
                    // CV is typically 0-1, but we allow it to boost slightly
                    self.level_cvs[idx] = value.clamp(0.0, 2.0);
                    self.level_cv_active[idx] = true;
                    return Ok(());
                }
            }
        }

        // Check for master
        if port == "master" {
            self.master_cv = value.clamp(0.0, 2.0);
            self.master_cv_active = true;
            return Ok(());
        }

        Err(format!("Unknown input port: {}", port))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "out" => Ok(self.mix()),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn reset_inputs(&mut self) {
        self.level_cv_active = [false; MAX_CHANNELS];
        self.master_cv_active = false;
        // Note: audio inputs don't have control fallbacks
    }

    fn controls(&self) -> Vec<ControlMeta> {
        let mut controls = Vec::with_capacity(self.channels + 1);

        for i in 0..self.channels {
            controls.push(
                ControlMeta::new(format!("level.{}", i), format!("Channel {} level", i + 1))
                    .with_range(0.0, 2.0)
                    .with_default(1.0),
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

        Err(format!("Unknown control: {}", key))
    }
}

/// Factory for constructing Mixer modules from configuration.
///
/// # Configuration Options
///
/// - `channels` (usize): Number of input channels, 1-8 (default: 4)
/// - `levels` (array of f32): Initial level for each channel (default: all 1.0)
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

        let master = config
            .get("master")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;

        let levels: Vec<f32> = config
            .get("levels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|n| n as f32))
                    .collect()
            })
            .unwrap_or_else(Vec::new);

        let controls = MixerControls::new_with_levels(channels, &levels, master);
        let mixer = Mixer::new_with_controls(channels, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(mixer)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls) as Arc<dyn Any + Send + Sync>,
            )],
            sink: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mixer_basic_summing() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 0.5).unwrap();
        mixer.set_input("in2", 0.3).unwrap();
        mixer.process();

        let out = mixer.get_output("out").unwrap();
        assert!((out - 0.8).abs() < 0.001, "Expected 0.8, got {}", out);
    }

    #[test]
    fn test_mixer_with_levels() {
        let mut mixer = Mixer::new(2)
            .with_level(0, 0.5) // Channel 1 at 50%
            .with_level(1, 1.0); // Channel 2 at 100%

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.process();

        let out = mixer.get_output("out").unwrap();
        assert!(
            (out - 1.5).abs() < 0.001,
            "Expected 1.5 (0.5 + 1.0), got {}",
            out
        );
    }

    #[test]
    fn test_mixer_master_level() {
        let mut mixer = Mixer::new(2).with_master(0.5);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.process();

        let out = mixer.get_output("out").unwrap();
        assert!(
            (out - 1.0).abs() < 0.001,
            "Expected 1.0 (2.0 * 0.5), got {}",
            out
        );
    }

    #[test]
    fn test_mixer_level_cv() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.set_input("level1", 0.5).unwrap(); // CV reduces channel 1
        mixer.set_input("level2", 1.0).unwrap();
        mixer.process();

        let out = mixer.get_output("out").unwrap();
        assert!(
            (out - 1.5).abs() < 0.001,
            "Expected 1.5 (0.5 + 1.0), got {}",
            out
        );
    }

    #[test]
    fn test_mixer_master_cv() {
        let mut mixer = Mixer::new(2);

        mixer.set_input("in1", 1.0).unwrap();
        mixer.set_input("in2", 1.0).unwrap();
        mixer.set_input("master", 0.25).unwrap();
        mixer.process();

        let out = mixer.get_output("out").unwrap();
        assert!(
            (out - 0.5).abs() < 0.001,
            "Expected 0.5 (2.0 * 0.25), got {}",
            out
        );
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
    }

    #[test]
    fn test_mixer_invalid_port() {
        let mut mixer = Mixer::new(2);

        // in3 doesn't exist on 2-channel mixer
        assert!(mixer.set_input("in3", 1.0).is_err());
        assert!(mixer.set_input("level3", 1.0).is_err());
        assert!(mixer.get_output("invalid").is_err());
    }

    #[test]
    fn test_mixer_factory() {
        let factory = MixerFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "mixer");

        let config = serde_json::json!({
            "channels": 3,
            "levels": [0.8, 0.6, 0.4],
            "master": 0.9
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Mixer");

        // Should have in1-in3, level1-level3, and master
        let inputs = module.inputs();
        assert_eq!(inputs.len(), 7); // 3 ins + 3 levels + 1 master
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

        let out = mixer.get_output("out").unwrap();
        assert!(
            out.abs() < 0.001,
            "Expected ~0 (signals cancel), got {}",
            out
        );
    }

    #[test]
    fn test_mixer_controls() {
        let mut mixer = Mixer::new(2);

        // Test control metadata
        let control_meta = Module::controls(&mixer);
        assert_eq!(control_meta.len(), 3); // level.0, level.1, master
        assert_eq!(control_meta[0].key, "level.0");
        assert_eq!(control_meta[1].key, "level.1");
        assert_eq!(control_meta[2].key, "master");

        // Test get/set controls
        mixer.set_control("level.0", 0.5).unwrap();
        assert_eq!(mixer.get_control("level.0").unwrap(), 0.5);

        mixer.set_control("master", 0.8).unwrap();
        assert_eq!(mixer.get_control("master").unwrap(), 0.8);

        // Test invalid control
        assert!(mixer.get_control("invalid").is_err());
        assert!(mixer.get_control("level.5").is_err()); // Only 2 channels
    }
}
