//! Mixer module for combining multiple audio signals.
//!
//! The mixer combines multiple input signals with individual level controls
//! and a master output level. This is essential for layering sounds, creating
//! complex timbres, and balancing multiple voices.
//!
//! # Features
//!
//! - 4 input channels (default, configurable 1-64)
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
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::MixerControls;

mod controls;
mod inputs;
mod outputs;

/// Maximum number of mixer channels supported.
pub const MAX_CHANNELS: usize = 64;

/// A multi-channel audio mixer.
///
/// Combines multiple audio inputs with individual level controls. Each channel
/// has both a base level (set via configuration) and a CV input for dynamic
/// control, plus a pan control with optional modulation. The final output is a
/// stereo pair summed across all channels and multiplied by the master level.
///
/// # Inputs
///
/// - `in1` through `in64` - Audio inputs (depending on channel count)
/// - `level1` through `level64` - Level CV inputs (multiplied with base level)
/// - `pan1` through `pan64` - Pan modulation inputs (added to base pan)
/// - `master` - Master output level CV
///
/// # Outputs
///
/// - `left` - Mixed left-channel output
/// - `right` - Mixed right-channel output
///
/// # Controls
///
/// - `level.0` through `level.63` - Per-channel base levels
/// - `pan.0` through `pan.63` - Per-channel base pan positions
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
    outputs: outputs::MixerOutputs,
}

impl Mixer {
    /// Creates a new mixer with the specified number of channels.
    ///
    /// Channel count is clamped to 1-64. Default levels are 1.0 (unity gain).
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
            outputs: outputs::MixerOutputs::new(),
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

    fn effective_pan(&self, channel: usize, i: usize) -> f32 {
        (self.ctrl.pan(channel) + self.input_state.pan_mod(channel, i)).clamp(-1.0, 1.0)
    }

    /// Mixes all inputs at frame `i` and returns the stereo output sample.
    fn mix(&self, i: usize) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;

        for ch in 0..self.channels {
            let level_cv = self.input_state.level_cv(ch, i);
            let channel_out = self.input_state.audio(ch, i) * self.ctrl.level(ch) * level_cv;
            let pan = self.effective_pan(ch, i);
            let angle = (pan + 1.0) * FRAC_PI_4;
            left += channel_out * angle.cos();
            right += channel_out * angle.sin();
        }

        let master_level = self.ctrl.master();
        let master_cv = self.input_state.master_cv(i);
        let gain = master_level * master_cv;

        (left * gain, right * gain)
    }
}

impl Module for Mixer {
    fn name(&self) -> &str {
        "Mixer"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let (left, right) = self.mix(i);
            self.outputs.set(i, left, right);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        self.input_state.names()
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.input_state.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.input_state.set(self.channels, port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.input_state.set_connected(index, connected);
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
/// - `channels` (usize): Number of input channels, 1-64 (default: 4)
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
            .unwrap_or_default();

        let pans: Vec<f32> = config
            .get("pans")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|n| n as f32))
                    .collect()
            })
            .unwrap_or_default();

        let controls = MixerControls::new_with_config(channels, &levels, &pans, master);
        let mixer = Mixer::new_with_controls(channels, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(mixer)),
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
mod tests;
