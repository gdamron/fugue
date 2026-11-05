//! Contour: one shaped ramp that works as an envelope, an LFO or an audio
//! oscillator, with four related outputs.
//!
//! A single rise and fall, its curve set by `shape`, its balance of attack
//! and decay by `slope`, smoothed or folded by `smoothness`. Triggered once
//! it is an envelope; looping it is an LFO, or at audio rate an oscillator.
//! `output_mode` and `shift` relate the four outputs to one another: gates
//! and a raw copy, a pan, phase offsets or frequency ratios, so one module
//! drives a chord of filters, VCAs or voices that move together.

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::LinearRamp;
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::ConfigKey;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::{
    FREQUENCY, FREQUENCY_MOD_DEPTH, MODE, OUTPUT_MODE, RANGE, SHAPE, SHAPE_MOD_DEPTH, SHIFT, SLOPE,
    SLOPE_MOD_DEPTH, SMOOTHNESS, SMOOTHNESS_MOD_DEPTH, TABLE,
};
use self::generator::{Modes, Params};
use self::ramp::{ClockFollower, CHANNELS};
use self::voice::{Sense, Voice};

mod controls;
mod generator;
mod ramp;
mod ratios;
mod shaper;
mod tables;
#[cfg(test)]
mod tests;
mod voice;

const TYPE_ID: &str = "contour";

/// Input port names, in `input_block_mut` order.
const INPUTS: [&str; 7] = [
    "trigger",
    "clock",
    "frequency",
    "frequency_mod",
    "shape_mod",
    "slope_mod",
    "smoothness_mod",
];
const TRIGGER_IN: usize = 0;
const CLOCK_IN: usize = 1;
const FREQUENCY_IN: usize = 2;
const FREQUENCY_MOD_IN: usize = 3;
const SHAPE_MOD_IN: usize = 4;
const SLOPE_MOD_IN: usize = 5;
const SMOOTHNESS_MOD_IN: usize = 6;

/// Output port names, in `output_block` order.
const OUTPUTS: [&str; CHANNELS] = ["contour.0", "contour.1", "contour.2", "contour.3"];

/// How long a write to `frequency`, `shape`, `slope`, `smoothness` or
/// `shift` takes to arrive. A write that landed at once would step every
/// output; a sweep of writes would be heard as zipper.
const GLIDE_SECONDS: f32 = 0.01;

/// Factory for constructing contours from configuration.
pub struct ContourFactory;

impl ModuleFactory for ContourFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // The curve tables are built here, off the audio thread.
        tables::force();
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Every config key is a control's starting value; the registry
        // refuses any other key.
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let module = Contour::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// One shaped ramp: an envelope, an LFO or an oscillator, with four
/// related outputs.
///
/// # Inputs
/// - `trigger`: a rising edge starts an `ad` envelope or restarts a loop;
///   in `ar` it is the gate
/// - `clock`: rising edges set the cycle: each restarts it, and the time
///   between the last two is its length (until two have arrived,
///   `frequency` rules). With `trigger` unpatched, each edge also starts
///   an `ad` or `ar` envelope
/// - `frequency`: cycles per second; when patched it overrides the control
/// - `frequency_mod`: added to the frequency, in units of
///   `frequency_mod_depth` Hz
/// - `shape_mod`, `slope_mod`, `smoothness_mod`: added to their control,
///   scaled by its `_mod_depth`
///
/// # Outputs
/// `contour.0` to `contour.3`: bipolar −1 to 1 while looping, 0 to 1 as
/// envelopes, gates 0 or 1. Looping in the `audio` range, the outputs run
/// 2 samples behind the inputs, for band-limiting.
///
/// # Controls
/// - `frequency`: cycles per second, 0.001 to 20000 Hz
/// - `shape`, `slope`, `smoothness`, `shift`: 0 to 1; a write glides over
///   10 ms
/// - `mode`, `range`, `output_mode`: a change crossfades over two cycles
///   (20 ms to 4 s)
/// - `frequency_mod_depth`, `shape_mod_depth`, `slope_mod_depth`,
///   `smoothness_mod_depth`: how far each modulation input reaches (0 to
///   1 for shape, slope and smoothness; invert the signal for the other
///   way)
pub struct Contour {
    sample_rate: f32,
    cells: Arc<ControlCells>,

    frequency: LinearRamp,
    shape: LinearRamp,
    slope: LinearRamp,
    smoothness: LinearRamp,
    shift: LinearRamp,
    frequency_mod_depth: f32,
    shape_mod_depth: f32,
    slope_mod_depth: f32,
    smoothness_mod_depth: f32,
    /// Whether a block has been rendered: writes before then (the config,
    /// a development's starting values) land at once.
    heard: bool,

    voice: Voice,
    clock: ClockFollower,
    trigger_high: bool,

    inputs: [[f32; MAX_BLOCK]; INPUTS.len()],
    connected: [bool; INPUTS.len()],
    outputs: [[f32; MAX_BLOCK]; CHANNELS],
}

impl Contour {
    /// A contour holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let rate = sample_rate.max(1) as f32;
        let glide = (rate * GLIDE_SECONDS).round().max(1.0) as u32;
        let ramp = |value| LinearRamp::new(value, glide);
        let modes = Modes {
            ramp: RampMode::Looping,
            output: OutputMode::Phase,
            range: Range::Control,
        };
        let mut module = Self {
            sample_rate: rate,
            cells,
            frequency: ramp(1.0),
            shape: ramp(0.5),
            slope: ramp(0.5),
            smoothness: ramp(0.5),
            shift: ramp(0.5),
            frequency_mod_depth: 1.0,
            shape_mod_depth: 1.0,
            slope_mod_depth: 1.0,
            smoothness_mod_depth: 1.0,
            heard: false,
            voice: Voice::new(tables::tables(), rate, modes),
            clock: ClockFollower::default(),
            trigger_high: false,
            inputs: [[0.0; MAX_BLOCK]; INPUTS.len()],
            connected: [false; INPUTS.len()],
            outputs: [[0.0; MAX_BLOCK]; CHANNELS],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = module.cells.load(index) {
                let _ = apply_declared(&mut module, index, value);
            }
        }
        module
    }

    /// Frame `i`'s settings, with the modulation inputs added.
    #[inline]
    fn params(&mut self, i: usize) -> Params {
        let control_hz = self.frequency.next_value();
        let mut hz = if self.connected[FREQUENCY_IN] {
            finite_or(self.inputs[FREQUENCY_IN][i], control_hz)
        } else {
            control_hz
        };
        if self.connected[FREQUENCY_MOD_IN] {
            hz += finite_or(self.inputs[FREQUENCY_MOD_IN][i], 0.0) * self.frequency_mod_depth;
        }
        let mut frequency = finite_or(hz / self.sample_rate, 0.0).max(0.0);
        if self.connected[CLOCK_IN] {
            if let Some(clocked) = self.clock.frequency() {
                frequency = clocked;
            }
        }
        let shape = self.shape.next_value();
        let slope = self.slope.next_value();
        let smoothness = self.smoothness.next_value();
        Params {
            frequency,
            shape: self.modulated(shape, SHAPE_MOD_IN, self.shape_mod_depth, i),
            slope: self.modulated(slope, SLOPE_MOD_IN, self.slope_mod_depth, i),
            smoothness: self.modulated(smoothness, SMOOTHNESS_MOD_IN, self.smoothness_mod_depth, i),
            shift: self.shift.next_value(),
        }
    }

    /// `base` moved by modulation input `port` at `depth`, kept to 0..1.
    #[inline]
    fn modulated(&self, base: f32, port: usize, depth: f32, i: usize) -> f32 {
        if !self.connected[port] {
            return base;
        }
        (base + finite_or(self.inputs[port][i], 0.0) * depth).clamp(0.0, 1.0)
    }

    /// What the trigger and clock inputs did at frame `i`.
    #[inline]
    fn sense(&mut self, i: usize) -> Sense {
        let trigger_patched = self.connected[TRIGGER_IN];
        let high = trigger_patched && self.inputs[TRIGGER_IN][i] > 0.5;
        let trigger_rose = high && !self.trigger_high;
        self.trigger_high = high;
        let (clock_rose, clock_high, clock_phase) = if self.connected[CLOCK_IN] {
            let value = self.inputs[CLOCK_IN][i];
            let (rose, phase) = self.clock.step(value);
            (rose, value > 0.5, phase)
        } else {
            (false, false, None)
        };
        Sense {
            trigger_patched,
            trigger_rose,
            trigger_high: high,
            clock_rose,
            clock_high,
            clock_phase,
        }
    }

    /// Asks for `modes`: at once before anything is heard, else through a
    /// crossfade.
    fn ask(&mut self, modes: Modes) {
        if self.heard {
            self.voice.modes = modes;
        } else {
            self.voice.restart(modes);
        }
    }

    /// Glides `ramp` to `value`, or puts it there before anything is heard.
    fn glide(heard: bool, ramp: &mut LinearRamp, value: f32) {
        if heard {
            ramp.set_target(value);
        } else {
            ramp.snap(value);
        }
    }
}

#[inline]
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

impl Module for Contour {
    fn name(&self) -> &str {
        "Contour"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        for i in 0..frames {
            let params = self.params(i);
            let sense = self.sense(i);
            let out = self.voice.render(&params, &sense);
            for (k, value) in out.iter().enumerate() {
                self.outputs[k][i] = *value;
            }
        }
        self.heard = true;
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
        &self.outputs[index.min(CHANNELS - 1)]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.inputs[index].fill(value);
        self.set_input_connected(index, true);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        OUTPUTS
            .iter()
            .position(|name| *name == port)
            .map(|k| self.outputs[k][0])
            .ok_or_else(|| format!("Unknown output port: {port}"))
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        if let Some(flag) = self.connected.get_mut(index) {
            *flag = connected;
        }
        if index == CLOCK_IN && !connected {
            self.clock.reset();
        }
        if index == TRIGGER_IN && !connected {
            self.trigger_high = false;
        }
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let heard = self.heard;
        let mut modes = self.voice.modes;
        match (control, value) {
            (FREQUENCY, RtValue::F32(hz)) => Self::glide(heard, &mut self.frequency, hz),
            (SHAPE, RtValue::F32(v)) => Self::glide(heard, &mut self.shape, v),
            (SLOPE, RtValue::F32(v)) => Self::glide(heard, &mut self.slope, v),
            (SMOOTHNESS, RtValue::F32(v)) => Self::glide(heard, &mut self.smoothness, v),
            (SHIFT, RtValue::F32(v)) => Self::glide(heard, &mut self.shift, v),
            (FREQUENCY_MOD_DEPTH, RtValue::F32(v)) => self.frequency_mod_depth = v,
            (SHAPE_MOD_DEPTH, RtValue::F32(v)) => self.shape_mod_depth = v,
            (SLOPE_MOD_DEPTH, RtValue::F32(v)) => self.slope_mod_depth = v,
            (SMOOTHNESS_MOD_DEPTH, RtValue::F32(v)) => self.smoothness_mod_depth = v,
            (MODE, RtValue::U32(position)) => {
                modes.ramp = match position {
                    0 => RampMode::Ad,
                    1 => RampMode::Looping,
                    2 => RampMode::Ar,
                    _ => return Err(Refusal::Invalid),
                };
                self.ask(modes);
            }
            (RANGE, RtValue::U32(position)) => {
                modes.range = match position {
                    0 => Range::Control,
                    1 => Range::Audio,
                    _ => return Err(Refusal::Invalid),
                };
                self.ask(modes);
            }
            (OUTPUT_MODE, RtValue::U32(position)) => {
                modes.output = match position {
                    0 => OutputMode::Envelope,
                    1 => OutputMode::Amplitude,
                    2 => OutputMode::Phase,
                    3 => OutputMode::Ratio,
                    _ => return Err(Refusal::Invalid),
                };
                self.ask(modes);
            }
            _ => return Err(Refusal::Unsupported),
        }
        Ok(value)
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

/// How the ramp runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RampMode {
    /// Attack then decay, once per trigger.
    Ad,
    /// Cycles continuously.
    Looping,
    /// Attack while the gate is high, release when it falls.
    Ar,
}

/// The speed the ramp is tuned for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Range {
    /// Envelopes and LFOs: asymmetric attack-decay curves.
    Control,
    /// Oscillators: band-limited, with symmetric curves.
    Audio,
}

/// What the four outputs carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// The shaped ramp, the raw ramp, and end-of-attack and end-of-cycle
    /// gates.
    Envelope,
    /// One ramp panned across the outputs.
    Amplitude,
    /// Copies offset in phase (or, for envelopes, in slope).
    Phase,
    /// Copies at related frequencies.
    Ratio,
}
