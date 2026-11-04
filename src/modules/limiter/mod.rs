//! Limiter: a lookahead brickwall for the end of a mix, the module around
//! [`LookaheadLimiter`].
//!
//! Put it before the `dac` when a generative piece or a live edit might
//! push the mix past full scale: peaks are held at the ceiling, and
//! anything below it passes untouched, only delayed by the lookahead.
//!
//! ```json
//! { "id": "limiter", "type": "limiter",
//!   "config": { "ceiling": -1.0, "release": 0.1, "lookahead": 0.005 } }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::LookaheadLimiter;
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::*;

mod controls;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "limiter";
const LOOKAHEAD_KEY: ConfigKey = ConfigKey::float("lookahead");
/// The lookahead when the config does not say, and its range, in seconds.
const DEFAULT_LOOKAHEAD: f32 = 0.005;
const MIN_LOOKAHEAD: f32 = 0.0005;
const MAX_LOOKAHEAD: f32 = 0.02;

/// How long a ceiling change glides, in seconds.
const CEILING_GLIDE: f32 = 0.02;

const PORTS: [&str; 2] = ["audio_left", "audio_right"];

/// Factory for constructing [`Limiter`] modules. Every control but
/// `reduction` is also a config key, and `lookahead` (seconds, 0.0005 to
/// 0.02, default 0.005) sets how far ahead it looks, which is also how far
/// the output trails the input.
pub struct LimiterFactory;

impl ModuleFactory for LimiterFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[LOOKAHEAD_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let lookahead = reader.float(&LOOKAHEAD_KEY)?.unwrap_or(DEFAULT_LOOKAHEAD);
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Keys that are no control are left to the registry, which refuses
        // them by name.
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let limiter = Limiter::with_cells(sample_rate, lookahead, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(limiter)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// The lookahead in whole frames: `seconds` clamped to its range, rounded,
/// at least one.
fn lookahead_frames(sample_rate: u32, seconds: f32) -> usize {
    let seconds = seconds.clamp(MIN_LOOKAHEAD, MAX_LOOKAHEAD);
    ((seconds * sample_rate as f32).round() as usize).max(1)
}

/// A stereo-linked lookahead brickwall limiter.
///
/// # Inputs
/// - `audio_left`, `audio_right`: the mix (a mono source can use
///   `audio_left` alone)
///
/// # Outputs
/// - `audio_left`, `audio_right`: each input delayed by the lookahead,
///   times one level shared by both sides; no sample exceeds the ceiling.
///   Below the ceiling the output is the delayed input bit for bit
///
/// # Controls
/// - `ceiling`: −24 to 0 dBFS (default −1): no output sample's magnitude
///   exceeds it. A change glides over 20 ms and, like the audio, is heard a
///   lookahead later
/// - `release`: 0.001 to 2 s (default 0.1): after a peak has passed, the
///   level recovers 63% of the way back in this time. The level also
///   moves no faster than the lookahead allows, so a release shorter than
///   the lookahead recovers in about a lookahead
/// - `reduction` (read-only): the most the limiter pulled down during the
///   last block, in dB (0 when idle)
///
/// The level comes down smoothly over the lookahead before a peak, so the
/// limiting does not click.
///
/// # Config
/// - `lookahead`: seconds (0.0005 to 0.02, default 0.005), rounded to whole
///   frames: 240 at 48 kHz
pub struct Limiter {
    limiter: LookaheadLimiter,
    cells: Arc<ControlCells>,
    inputs: [[f32; MAX_BLOCK]; 2],
    outputs: [[f32; MAX_BLOCK]; 2],
}

impl Limiter {
    /// A limiter at `sample_rate` looking `lookahead` seconds ahead, with
    /// every control at its default.
    pub fn new(sample_rate: u32, lookahead: f32) -> Self {
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        Self::with_cells(sample_rate, lookahead, cells)
    }

    /// A limiter holding what `cells` hold, settled there.
    fn with_cells(sample_rate: u32, lookahead: f32, cells: Arc<ControlCells>) -> Self {
        let limiter = LookaheadLimiter::new(
            sample_rate,
            lookahead_frames(sample_rate, lookahead),
            decibels_to_level(DEFAULT_CEILING),
            (CEILING_GLIDE * sample_rate as f32).round() as u32,
            DEFAULT_RELEASE,
        );
        let mut module = Self {
            limiter,
            cells,
            inputs: [[0.0; MAX_BLOCK]; 2],
            outputs: [[0.0; MAX_BLOCK]; 2],
        };
        for index in [CEILING, RELEASE] {
            if let Some(value) = module.cells.load(index) {
                let _ = apply_declared(&mut module, index, value);
            }
        }
        module.limiter.settle();
        module
    }

    /// How many frames the output trails the input.
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }
}

/// The linear level of `decibels`. Control rate only.
fn decibels_to_level(decibels: f32) -> f32 {
    10f32.powf(decibels / 20.0)
}

impl Module for Limiter {
    fn name(&self) -> &str {
        "Limiter"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        let [in_left, in_right] = &self.inputs;
        let [out_left, out_right] = &mut self.outputs;
        let mut least = 1.0f32;
        for i in 0..frames {
            let (left, right, gain) = self.limiter.process(in_left[i], in_right[i]);
            out_left[i] = left;
            out_right[i] = right;
            least = least.min(gain);
        }
        let reduction = if least < 1.0 {
            (-20.0 * least.log10()).min(MAX_REDUCTION)
        } else {
            0.0
        };
        self.cells.publish(REDUCTION, RtValue::F32(reduction));
        true
    }

    fn inputs(&self) -> &[&str] {
        &PORTS
    }

    fn outputs(&self) -> &[&str] {
        &PORTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.inputs[index.min(1)]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.outputs[index.min(1)]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = PORTS.iter().position(|p| *p == port);
        let index = index.ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.inputs[index].fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = PORTS.iter().position(|p| *p == port);
        let index = index.ok_or_else(|| format!("Unknown output port: {port}"))?;
        Ok(self.outputs[index][0])
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (CEILING, RtValue::F32(v)) => {
                let v = v.clamp(MIN_CEILING, MAX_CEILING);
                self.limiter.set_ceiling(decibels_to_level(v));
                Ok(RtValue::F32(v))
            }
            (RELEASE, RtValue::F32(v)) => {
                let v = v.clamp(MIN_RELEASE, MAX_RELEASE);
                self.limiter.set_release(v);
                Ok(RtValue::F32(v))
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
