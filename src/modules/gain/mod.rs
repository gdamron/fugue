//! Gain: a level in decibels that can boost as well as cut.
//!
//! `vca` scales by a level within [0, 1]; `gain` works in dB, from −96 to
//! +24, so it can lift a signal too: a compressor's makeup, a quiet
//! sample brought up, a send trimmed. Its `decibels_mod` input adds dB at
//! audio rate, so a `gain_curve` patched into it turns it into the gain
//! stage of a compressor.
//!
//! ```json
//! { "id": "makeup", "type": "gain", "config": { "decibels": 6.0 } }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{decibels_to_level, LinearRamp, FLOOR_DECIBELS};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::ConfigKey;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::*;

mod controls;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "gain";
const INPUTS: [&str; 2] = ["audio", "decibels_mod"];
const OUTPUTS: [&str; 1] = ["audio"];
/// How long `invert` crossfades, in seconds.
const INVERT_FADE: f32 = 0.02;
/// How long a number control's change glides, in seconds: about one
/// 128-frame block at 48 kHz.
const CONTROL_GLIDE: f32 = 0.0027;

/// The frames a dynamics atom's number control glides over at
/// `sample_rate`. The glide is timed rather than spread over the block, so
/// it holds when a feedback loop runs the atom a frame at a time.
pub(crate) fn control_glide_frames(sample_rate: u32) -> u32 {
    (CONTROL_GLIDE * sample_rate as f32).round().max(1.0) as u32
}

/// Factory for constructing [`Gain`] modules. Every control is also a
/// config key.
pub struct GainFactory;

impl ModuleFactory for GainFactory {
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
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Keys that are no control are left to the registry, which refuses
        // them by name.
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let gain = Gain::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(gain)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A gain in dB, −96 to +24, with audio-rate modulation in dB.
///
/// # Inputs
/// - `audio`: the signal
/// - `decibels_mod`: dB added to `decibels` at each sample, times
///   `decibels_mod_depth` (patch a `gain_curve`'s `decibels` here)
///
/// # Outputs
/// - `audio`: the signal times the gain (the gain with its modulation is
///   held within −120 to +24 dB)
///
/// # Controls
/// - `decibels`: the gain, −96 to +24 dB (default 0, which passes the
///   signal bit for bit). A change glides over about 2.7 ms
/// - `decibels_mod_depth`: dB per unit of `decibels_mod`, 0 to 96 (default
///   1). A change glides over about 2.7 ms
/// - `invert`: flips the output's sign, crossfading over 20 ms
pub struct Gain {
    cells: Arc<ControlCells>,
    decibels_ramp: LinearRamp,
    depth_ramp: LinearRamp,
    sign: LinearRamp,
    audio: [f32; MAX_BLOCK],
    modulation: [f32; MAX_BLOCK],
    output: [f32; MAX_BLOCK],
}

impl Gain {
    /// A gain at `sample_rate` with every control at its default.
    pub fn new(sample_rate: u32) -> Self {
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        Self::with_cells(sample_rate, cells)
    }

    /// A gain holding what `cells` hold, settled there.
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut gain = Self {
            cells,
            decibels_ramp: LinearRamp::new(0.0, control_glide_frames(sample_rate)),
            depth_ramp: LinearRamp::new(1.0, control_glide_frames(sample_rate)),
            sign: LinearRamp::new(1.0, (INVERT_FADE * sample_rate as f32).round() as u32),
            audio: [0.0; MAX_BLOCK],
            modulation: [0.0; MAX_BLOCK],
            output: [0.0; MAX_BLOCK],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = gain.cells.load(index) {
                let _ = apply_declared(&mut gain, index, value);
            }
        }
        gain.decibels_ramp.settle();
        gain.depth_ramp.settle();
        gain.sign.settle();
        gain
    }
}

impl Module for Gain {
    fn name(&self) -> &str {
        "Gain"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        let signal = self.audio[..frames].iter().zip(&self.modulation[..frames]);
        for (out, (&x, &m)) in self.output[..frames].iter_mut().zip(signal) {
            let (db, depth) = (
                self.decibels_ramp.next_value(),
                self.depth_ramp.next_value(),
            );
            // Modulation that is not a number adds nothing: a `gain_curve`
            // never sends one, and silencing the signal would be worse.
            let m = if m.is_finite() { m } else { 0.0 };
            let db = (db + m * depth).clamp(FLOOR_DECIBELS, MAX_DECIBELS);
            let x = if x.is_finite() { x } else { 0.0 };
            *out = x * decibels_to_level(db) * self.sign.next_value();
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.audio,
            _ => &mut self.modulation,
        }
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.output
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS.iter().position(|p| *p == port);
        let index = index.ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.input_block_mut(index).fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.output[0]),
            _ => Err(format!("Unknown output port: {port}")),
        }
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (DECIBELS, RtValue::F32(v)) => {
                let decibels = v.clamp(MIN_DECIBELS, MAX_DECIBELS);
                self.decibels_ramp.set_target(decibels);
                Ok(RtValue::F32(decibels))
            }
            (DECIBELS_MOD_DEPTH, RtValue::F32(v)) => {
                let depth = v.clamp(0.0, MAX_DEPTH);
                self.depth_ramp.set_target(depth);
                Ok(RtValue::F32(depth))
            }
            (INVERT, RtValue::Bool(on)) => {
                self.sign.set_target(if on { -1.0 } else { 1.0 });
                Ok(value)
            }
            _ => Err(Refusal::Unsupported),
        }
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
