//! Granular: a live texture processor that records what is patched into
//! it and plays it back as a cloud of grains.
//!
//! It turns a short gesture into a backdrop: `freeze` stops recording and
//! keeps playing what the buffer holds; `position`, `size` and `pitch`
//! choose where grains read, how long they are and how they are
//! transposed; `density` and `texture` set how many there are and how
//! their windows are shaped; `spread` scatters them across the stereo
//! field. A `trigger` starts a grain at once, and with `sync` on a patched
//! `clock` places grains on its divisions, so the texture can pulse with
//! the music. The output is the processed signal alone: mix it with the
//! dry signal outside (the `fugue.effects.granular` development does, with
//! a reverb send).
//!
//! The feedback path and the density and texture mappings are ported from
//! Mutable Instruments' `clouds/dsp/granular_processor.cc` (MIT, Copyright
//! 2014 Emilie Gillet), and the soft limit from stmlib's `dsp/dsp.h` (MIT,
//! Copyright 2012 Emilie Gillet); the recording, grains and smear come from
//! [`crate::dsp::GrainCloud`] and its neighbours, which say what else was
//! ported from where. See THIRD_PARTY_NOTICES.md.
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "cloud", "type": "granular", "config": { "buffer_length": 4.0, "density": 0.8, "texture": 0.7, "spread": 0.6 } }
//!   ]
//! }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{
    share_per_frame, ClockDivider, Diffuser, GrainShape, LinearRamp, OnePole, Onsets, Recording,
    RisingEdge,
};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::delay::soft_clip;
use crate::modules::mixer::gain_ramp_frames;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::{
    divisions, DENSITY, FEEDBACK, FREEZE, GLIDE, INERTIA, LONGEST_GLIDE, MODE, MOST_PITCH, PITCH,
    POSITION, SIZE, SPREAD, SYNC, TABLE, TEXTURE,
};
use self::playback::{Frame, Mode, Playback};

mod controls;
mod playback;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "granular";
const BUFFER_LENGTH_KEY: ConfigKey = ConfigKey::float("buffer_length");
const SEED_KEY: ConfigKey = ConfigKey::seed("seed");
/// The buffer's length when config doesn't say, and its longest, in seconds.
const DEFAULT_BUFFER_LENGTH: f32 = 4.0;
const LONGEST_BUFFER: f32 = 60.0;
/// Enough frames for a grain to read clear of the write head.
const SHORTEST_BUFFER_FRAMES: usize = 64;

const INPUTS: [&str; 4] = ["audio_left", "audio_right", "trigger", "clock"];
const RIGHT_IN: usize = 1;
const TRIGGER_IN: usize = 2;
const CLOCK_IN: usize = 3;
const OUTPUTS: [&str; 2] = ["audio_left", "audio_right"];

/// Between these `density` values, no grains start on their own.
const SILENT_BELOW: f32 = 0.47;
const SILENT_ABOVE: f32 = 0.53;

/// The feedback path's soft limit: unchanged near 0, reaching ±1 at ±3.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// Factory for constructing granular processors from configuration.
pub struct GranularFactory;

impl ModuleFactory for GranularFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[BUFFER_LENGTH_KEY, SEED_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let length = reader
            .float(&BUFFER_LENGTH_KEY)?
            .unwrap_or(DEFAULT_BUFFER_LENGTH);
        if length.is_nan() || length <= 0.0 {
            return Err(format!(
                "granular config 'buffer_length' must be above 0 seconds, got {length}"
            )
            .into());
        }
        let seed = reader.int::<u64>(&SEED_KEY)?.unwrap_or(0);
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let module = Granular::with_cells(sample_rate, length, seed, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A stereo granular processor over a circular recording.
///
/// # Inputs
/// - `audio_left`, `audio_right`: the signal to record; with `audio_right`
///   unpatched, both channels record `audio_left`
/// - `trigger`: a rising edge starts a grain on that frame (granular),
///   cuts to a fresh window at `position` (stretch), or taps the delay's
///   length, the time between two taps (looping delay)
/// - `clock`: gates whose divisions place grains, or whose period sets
///   the looping delay, while `sync` is on
///
/// # Outputs
/// - `audio_left`, `audio_right`: the grains alone, within ±1
///
/// # Controls
/// - `mode`: `granular`, `stretch` or `looping_delay` (a change
///   crossfades over 20 ms)
/// - `position`, `size`, `pitch`, `density`, `texture`, `spread`: the
///   grains (each grain keeps the values it started with, so changes
///   never step the sound)
/// - `freeze`: stop recording and keep playing the buffer; the writes
///   fade out and back in over 20 ms, so neither edge clicks
/// - `feedback`: output fed back into the recording, through a high-pass
///   and a soft limit; it fades out while frozen
/// - `sync`: grains start on `clock` divisions instead of by `density`
///   alone
/// - `glide`: looping delay only, seconds a moved `position` takes to
///   slide to its new delay, tape-style (0, the default, crossfades)
/// - `inertia`: while gliding, the tape's mass: heavier, the read speed
///   changes more slowly and a move bends longer
///
/// # Config
/// - `buffer_length`: seconds recorded (default 4; above 0, held to 60),
///   allocated once at build
/// - `seed`: the grains' random stream (default 0), so renders repeat
pub struct Granular {
    cells: Arc<ControlCells>,
    recording: Recording,
    playback: Playback,
    diffuser: Diffuser,

    shape: GrainShape,
    pitch: f32,
    density: f32,
    texture: f32,
    freeze: bool,
    /// Whether the last block was frozen, to catch recording restarting.
    was_frozen: bool,
    sync: bool,

    feedback: LinearRamp,
    diffusion: LinearRamp,
    /// Follows `freeze` slowly, so feedback fades out while frozen.
    frozen: f32,
    frozen_step: f32,
    feedback_filters: [OnePole; 2],
    last: (f32, f32),

    trigger: RisingEdge,
    clock: ClockDivider,
    inputs: [[f32; MAX_BLOCK]; 4],
    connected: [bool; 4],
    out: [[f32; MAX_BLOCK]; 2],
}

impl Granular {
    fn with_cells(sample_rate: u32, length: f32, seed: u64, cells: Arc<ControlCells>) -> Self {
        let rate = sample_rate.max(1) as f32;
        let length = length.min(LONGEST_BUFFER);
        let glide = gain_ramp_frames(sample_rate);
        let mut module = Self {
            cells,
            recording: Recording::new(
                sample_rate,
                ((length * rate).round() as usize).max(SHORTEST_BUFFER_FRAMES),
            ),
            playback: Playback::new(sample_rate, seed, glide),
            diffuser: Diffuser::new(sample_rate),
            shape: GrainShape {
                ratio: 1.0,
                ..GrainShape::default()
            },
            pitch: 0.0,
            density: 0.7,
            texture: 0.5,
            freeze: false,
            was_frozen: false,
            sync: false,
            feedback: LinearRamp::new(0.0, glide),
            diffusion: LinearRamp::new(0.0, glide),
            frozen: 0.0,
            frozen_step: share_per_frame(0.0005, rate),
            feedback_filters: [
                OnePole::new(sample_rate, 20.0),
                OnePole::new(sample_rate, 20.0),
            ],
            last: (0.0, 0.0),
            trigger: RisingEdge::default(),
            clock: ClockDivider::default(),
            inputs: [[0.0; MAX_BLOCK]; 4],
            connected: [false; 4],
            out: [[0.0; MAX_BLOCK]; 2],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = module.cells.load(index) {
                let _ = apply_declared(&mut module, index, value);
            }
        }
        // Built, the processor starts where its controls are.
        module.playback.settle();
        module.update_diffusion();
        module.feedback.settle();
        module.diffusion.settle();
        module.frozen = if module.freeze { 1.0 } else { 0.0 };
        module.was_frozen = module.freeze;
        module
    }

    /// The grains' window and the other modes' tone, from `texture`.
    fn set_texture(&mut self, texture: f32) {
        self.texture = texture;
        self.shape.window = (texture / 0.75).min(1.0);
        self.playback.set_texture(texture);
        self.update_diffusion();
    }

    /// The smear: from `texture` above 0.75 for grains, else `density`.
    fn update_diffusion(&mut self) {
        let amount = match self.playback.mode() {
            Mode::Granular => ((self.texture - 0.75) * 4.0).clamp(0.0, 1.0),
            Mode::Stretch | Mode::LoopingDelay => self.density,
        };
        self.diffusion.set_target(amount);
    }

    /// How far `density` is past its silent middle, 0 to 1 (0 within it).
    fn reach(&self) -> f32 {
        let past = if self.density >= SILENT_ABOVE {
            self.density - SILENT_ABOVE
        } else if self.density <= SILENT_BELOW {
            SILENT_BELOW - self.density
        } else {
            0.0
        };
        (past / SILENT_BELOW).min(1.0)
    }

    fn run(&mut self, frames: usize) {
        let feedback = self.feedback.value();
        for filter in &mut self.feedback_filters {
            filter.set_cutoff(20.0 + 100.0 * feedback * feedback);
        }
        let reach = self.reach();
        // As the original: 2.12 per unit of density past the middle.
        let overlap = (reach * SILENT_BELOW * 2.12).min(1.0);
        let regular = self.density < 0.5;
        let divisions = divisions(reach);
        let synced = self.sync && self.connected[CLOCK_IN];
        let recording = !self.freeze;
        if recording && self.was_frozen {
            self.playback.thaw(&self.recording);
        }
        self.playback.begin_block();
        self.was_frozen = self.freeze;
        for i in 0..frames {
            let left = finite(self.inputs[0][i]);
            let right = if self.connected[RIGHT_IN] {
                finite(self.inputs[RIGHT_IN][i])
            } else {
                left
            };
            self.frozen += self.frozen_step * (f32::from(u8::from(self.freeze)) - self.frozen);
            let gain = self.feedback.next_value() * (1.0 - self.frozen);
            let fed_left = self.feedback_filters[0].highpass(self.last.0);
            let fed_right = self.feedback_filters[1].highpass(self.last.1);
            let left = left + gain * (soft_limit(gain * 1.4 * fed_left + left) - left);
            let right = right + gain * (soft_limit(gain * 1.4 * fed_right + right) - right);
            self.recording.write(left, right, recording);

            let trigger = self.trigger.rose(self.inputs[TRIGGER_IN][i]);
            let onset = self.clock.tick(self.inputs[CLOCK_IN][i], divisions);
            let onsets = if synced {
                Onsets::Clock(onset)
            } else {
                Onsets::Density
            };
            let (left, right) = self.playback.tick(&Frame {
                recording: &self.recording,
                shape: &self.shape,
                overlap,
                regular,
                onsets,
                trigger,
                period: if synced { self.clock.period() } else { None },
                recording_on: recording,
            });
            let (left, right) = self.diffuser.tick(left, right, self.diffusion.next_value());
            self.last = (left, right);
            self.out[0][i] = soft_clip(left).clamp(-1.0, 1.0);
            self.out[1][i] = soft_clip(right).clamp(-1.0, 1.0);
        }
    }
}

/// `x`, or silence if it is not a finite number.
#[inline]
fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

impl Module for Granular {
    fn name(&self) -> &str {
        "Granular"
    }

    fn process(&mut self, frames: usize) -> bool {
        self.run(frames.min(MAX_BLOCK));
        true
    }

    fn inputs(&self) -> &[&str] {
        &INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.inputs[index.min(INPUTS.len() - 1)]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.out[index.min(1)]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.inputs[index].fill(value);
        self.connected[index] = true;
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = OUTPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown output port: {port}"))?;
        Ok(self.out[index][0])
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        if let Some(flag) = self.connected.get_mut(index) {
            *flag = connected;
        }
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let unit = |x: f32| x.clamp(0.0, 1.0);
        let applied = match (control, value) {
            (MODE, RtValue::U32(index)) => {
                let mode = Mode::from_index(index).ok_or(Refusal::Unsupported)?;
                self.playback.request(mode);
                self.update_diffusion();
                value
            }
            (POSITION, RtValue::F32(x)) => {
                // Turned, `position` sets the delay again after taps.
                if unit(x) != self.shape.position {
                    self.playback.forget_taps();
                }
                self.shape.position = unit(x);
                RtValue::F32(self.shape.position)
            }
            (SIZE, RtValue::F32(x)) => {
                self.shape.size = unit(x);
                RtValue::F32(self.shape.size)
            }
            (PITCH, RtValue::F32(x)) => {
                self.pitch = x.clamp(-MOST_PITCH, MOST_PITCH);
                self.shape.ratio = (self.pitch / 12.0).exp2();
                RtValue::F32(self.pitch)
            }
            (DENSITY, RtValue::F32(x)) => {
                self.density = unit(x);
                self.update_diffusion();
                RtValue::F32(self.density)
            }
            (TEXTURE, RtValue::F32(x)) => {
                self.set_texture(unit(x));
                RtValue::F32(self.texture)
            }
            (SPREAD, RtValue::F32(x)) => {
                self.shape.spread = unit(x);
                RtValue::F32(self.shape.spread)
            }
            (FEEDBACK, RtValue::F32(x)) => {
                self.feedback.set_target(unit(x));
                RtValue::F32(unit(x))
            }
            (FREEZE, RtValue::Bool(on)) => {
                self.freeze = on;
                value
            }
            (SYNC, RtValue::Bool(on)) => {
                self.sync = on;
                value
            }
            (GLIDE, RtValue::F32(x)) => {
                let seconds = x.clamp(0.0, LONGEST_GLIDE);
                self.playback.set_glide(seconds);
                RtValue::F32(seconds)
            }
            (INERTIA, RtValue::F32(x)) => {
                self.playback.set_inertia(unit(x));
                RtValue::F32(unit(x))
            }
            _ => return Err(Refusal::Unsupported),
        };
        Ok(applied)
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}
