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
//! - Level and master changes glide over [`GAIN_RAMP_MS`], starting on the
//!   frame the change lands, so moving a fader never clicks; the level and
//!   master CV inputs stay sample-exact, for gating
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc1", "type": "oscillator", "config": { "waveform": "sawtooth" } },
//!     { "id": "osc2", "type": "oscillator", "config": { "waveform": "square" } },
//!     {
//!       "id": "mixer",
//!       "type": "mixer",
//!       "config": { "channel_count": 2, "levels": [0.5, 0.3], "pans": [-0.3, 0.3] }
//!     },
//!     { "id": "dac", "type": "dac" }
//!   ],
//!   "connections": [
//!     { "from": "osc1", "from_port": "audio", "to": "mixer", "to_port": "audio.0" },
//!     { "from": "osc2", "from_port": "audio", "to": "mixer", "to_port": "audio.1" },
//!     { "from": "mixer", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
//!     { "from": "mixer", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
//!   ]
//! }
//! ```

use std::any::Any;
use std::f32::consts::FRAC_PI_4;
use std::sync::Arc;

use crate::dsp::LinearRamp;
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::MixerControls;

mod controls;
mod inputs;
mod outputs;

/// Maximum number of mixer channels supported.
pub const MAX_CHANNELS: usize = 64;

/// How long a level or master change takes to arrive. A gain that changes
/// between two samples steps the waveform, which is heard as a click (FUG-258
/// measured them on every fast fader move, on clicking a fader's track and on
/// a cancelled drag's restore). Ten milliseconds removes the step and is
/// still shorter than one frame of a gesture at 60 Hz, so a moving fader never
/// falls behind.
pub const GAIN_RAMP_MS: f32 = 10.0;

/// The sample rate a mixer assumes until told otherwise
/// ([`Mixer::with_sample_rate`]).
pub(crate) const DEFAULT_SAMPLE_RATE: u32 = 48_000;

const MS_PER_SECOND: f32 = 1_000.0;

/// [`GAIN_RAMP_MS`] in frames at `sample_rate`.
pub fn gain_ramp_frames(sample_rate: u32) -> u32 {
    (sample_rate as f32 * GAIN_RAMP_MS / MS_PER_SECOND).round() as u32
}

/// A multi-channel audio mixer.
///
/// Combines multiple audio inputs with individual level controls. Each channel
/// has both a base level (set via configuration) and a CV input for dynamic
/// control, plus a pan control with optional modulation. The final output is a
/// stereo pair summed across all channels and multiplied by the master level.
///
/// # Inputs
///
/// - `audio.0` through `audio.63` - Audio inputs (depending on channel count)
/// - `level.0` through `level.63` - Level CV inputs (multiplied with base level)
/// - `pan.0` through `pan.63` - Pan modulation inputs (added to base pan)
/// - `master` - Master output level CV
///
/// # Outputs
///
/// - `audio_left` - Mixed left-channel output
/// - `audio_right` - Mixed right-channel output
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
    /// The levels and master the audio hears, gliding to the controls'.
    levels: [LinearRamp; MAX_CHANNELS],
    master: LinearRamp,
    /// False until the first block, which starts at the controls' values.
    started: bool,
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
        let frames = gain_ramp_frames(DEFAULT_SAMPLE_RATE);

        Self {
            channels,
            ctrl: controls,
            levels: [LinearRamp::new(0.0, frames); MAX_CHANNELS],
            master: LinearRamp::new(0.0, frames),
            started: false,
            input_state: inputs::MixerInputs::new(channels),
            outputs: outputs::MixerOutputs::new(),
        }
    }

    /// Times the level and master glides for `sample_rate`. The next block
    /// starts at the controls' values, as the first one does.
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        let frames = gain_ramp_frames(sample_rate);
        self.levels = [LinearRamp::new(0.0, frames); MAX_CHANNELS];
        self.master = LinearRamp::new(0.0, frames);
        self.started = false;
        self
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

    /// Reads the controls once a block: a changed level or master starts a
    /// glide; the first block starts at the controls' values.
    fn follow_controls(&mut self) {
        for ch in 0..self.channels {
            let level = self.ctrl.level(ch);
            if self.started {
                self.levels[ch].set_target(level);
            } else {
                self.levels[ch].snap(level);
            }
        }
        let master = self.ctrl.master();
        if self.started {
            self.master.set_target(master);
        } else {
            self.master.snap(master);
        }
        self.started = true;
    }

    /// Mixes all inputs at frame `i` and returns the stereo output sample.
    fn mix(&mut self, i: usize) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;

        for ch in 0..self.channels {
            let level_cv = self.input_state.level_cv(ch, i);
            let level = self.levels[ch].next_value();
            let channel_out = self.input_state.audio(ch, i) * level * level_cv;
            let pan = self.effective_pan(ch, i);
            let angle = (pan + 1.0) * FRAC_PI_4;
            left += channel_out * angle.cos();
            right += channel_out * angle.sin();
        }

        let master_level = self.master.next_value();
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
        self.follow_controls();
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
/// - `channel_count` (usize): Number of input channels, 1-64 (default: 4)
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
///     "channel_count": 4,
///     "levels": [0.8, 0.6, 0.4, 0.3],
///     "pans": [-0.5, -0.15, 0.15, 0.5],
///     "master": 0.8
///   }
/// }
/// ```
pub struct MixerFactory;

const TYPE_ID: &str = "mixer";
const CHANNEL_COUNT: ConfigKey = ConfigKey::int::<usize>("channel_count");
const MASTER: ConfigKey = ConfigKey::float("master");
const LEVELS: ConfigKey = ConfigKey::json("levels");
const PANS: ConfigKey = ConfigKey::json("pans");

impl ModuleFactory for MixerFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[CHANNEL_COUNT, MASTER, LEVELS, PANS] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let channels = reader.int::<usize>(&CHANNEL_COUNT)?.unwrap_or(4);
        let master = reader.float(&MASTER)?.unwrap_or(1.0);
        let levels = reader.floats(&LEVELS)?.unwrap_or_default();
        let pans = reader.floats(&PANS)?.unwrap_or_default();

        let controls = MixerControls::new_with_config(channels, &levels, &pans, master);
        crate::factory::apply_control_keys(&controls, config, |key| {
            key.starts_with("level.") || key.starts_with("pan.")
        })?;
        let mixer =
            Mixer::new_with_controls(channels, controls.clone()).with_sample_rate(sample_rate);

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
