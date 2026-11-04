//! Drive: saturation, distortion and lo-fi reduction, the module around
//! [`DriveProcessor`].
//!
//! From a little warmth on a bass to a fuzz or a crushed, early-sampler
//! grit. Curves run oversampled (and anti-aliased at their corners), so
//! even hard settings stay free of the inharmonic whine of aliasing; bits
//! and rate reduction alias on purpose and run at the base rate.
//!
//! ```json
//! { "id": "drive", "type": "drive",
//!   "config": { "shape": "asymmetric", "drive": 0.6, "post_tone": -0.3 } }
//! ```
//!
//! Its defaults suit a building block in a feedback loop (a tape delay's
//! saturation): fully wet, mild drive, automatic level on.

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{DriveProcessor, DriveShape, Factor};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::mixer::gain_ramp_frames;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::*;

mod controls;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "drive";
const OVERSAMPLING_KEY: ConfigKey = ConfigKey::integer("oversampling", 1, 4);
/// Oversampling when the config does not say.
const DEFAULT_OVERSAMPLING: u32 = 4;

/// Factory for constructing [`Drive`] modules. Every control is also a
/// config key, and `oversampling` (1, 2 or 4) sets how far the curves'
/// rate is raised (3 rounds up to 4).
pub struct DriveFactory;

impl ModuleFactory for DriveFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[OVERSAMPLING_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let oversampling = reader
            .int::<u32>(&OVERSAMPLING_KEY)?
            .unwrap_or(DEFAULT_OVERSAMPLING);
        let factor = oversampling_factor(oversampling);
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Keys that are no control are left to the registry, which refuses
        // them by name.
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let drive = Drive::with_cells(sample_rate, factor, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(drive)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// The oversampler for an `oversampling` setting (1 to 4): none for 1;
/// 3, which the halfband stages cannot make, rounds up to 4.
fn oversampling_factor(oversampling: u32) -> Option<Factor> {
    match oversampling {
        0 | 1 => None,
        2 => Some(Factor::Two),
        _ => Some(Factor::Four),
    }
}

/// Saturation, distortion and bit or rate reduction of one audio signal.
///
/// # Inputs
/// - `audio`: the signal to drive
///
/// # Outputs
/// - `audio`: the driven signal, within ±1. With oversampling it trails
///   the input by a few frames (the dry share is delayed to match, so the
///   mix never combs).
///
/// # Controls
/// - `drive`: 0 to 1. For the curves, the gain in front of them (0 dB to
///   +24 dB, +18 dB for fold); for `bits`, 16 bits down to 3; for `rate`,
///   every sample kept down to one in 64
/// - `shape`: `soft` (tanh-like), `hard` (clip), `fold` (foldback),
///   `asymmetric` (even harmonics), `bits`, `rate`. A change crossfades
///   over 20 ms
/// - `pre_tone`, `post_tone`: tilts (−1 dark to 1 bright, ±6 dB about
///   800 Hz) before and after the shape
/// - `bias`: an offset into the shape; off centre adds even harmonics, and
///   a DC blocker after the shape removes the offset it leaves
/// - `level`: output level, 0 to 1
/// - `auto_level`: on, a signal peaking at −12 dBFS keeps its level as
///   `drive` moves; louder signals come out quieter as they saturate
/// - `mix`: 0 dry to 1 fully driven
///
/// Every number glides and every switch crossfades, so nothing clicks.
///
/// # Config
/// - `oversampling`: 1, 2 or 4 (the default) times the rate for the
///   curves; 3 rounds up to 4
pub struct Drive {
    processor: DriveProcessor,
    cells: Arc<ControlCells>,
    input: [f32; MAX_BLOCK],
    output: [f32; MAX_BLOCK],
}

impl Drive {
    /// A drive at `sample_rate` with every control at its default,
    /// oversampled by `factor` (`None` for none).
    pub fn new(sample_rate: u32, factor: Option<Factor>) -> Self {
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        Self::with_cells(sample_rate, factor, cells)
    }

    /// A drive holding what `cells` hold, settled there.
    fn with_cells(sample_rate: u32, factor: Option<Factor>, cells: Arc<ControlCells>) -> Self {
        let processor = DriveProcessor::new(sample_rate, factor, gain_ramp_frames(sample_rate));
        let mut drive = Self {
            processor,
            cells,
            input: [0.0; MAX_BLOCK],
            output: [0.0; MAX_BLOCK],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = drive.cells.load(index) {
                let _ = apply_declared(&mut drive, index, value);
            }
        }
        drive.processor.settle();
        drive
    }

    /// How many frames the output trails the input.
    pub fn latency(&self) -> usize {
        self.processor.latency()
    }
}

impl Module for Drive {
    fn name(&self) -> &str {
        "Drive"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        self.processor
            .process(&self.input[..frames], &mut self.output[..frames]);
        true
    }

    fn inputs(&self) -> &[&str] {
        &["audio"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.input
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.output
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.input.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {port}")),
        }
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
        let d = &mut self.processor;
        let applied = match (control, value) {
            (DRIVE, RtValue::F32(v)) => applied(v.clamp(0.0, 1.0), |v| d.set_drive(v)),
            (SHAPE, RtValue::U32(position)) => {
                let shape = DriveShape::ALL.get(position as usize);
                d.set_shape(*shape.ok_or(Refusal::Invalid)?);
                value
            }
            (PRE_TONE, RtValue::F32(v)) => applied(v.clamp(-1.0, 1.0), |v| d.set_pre_tone(v)),
            (POST_TONE, RtValue::F32(v)) => applied(v.clamp(-1.0, 1.0), |v| d.set_post_tone(v)),
            (BIAS, RtValue::F32(v)) => applied(v.clamp(-1.0, 1.0), |v| d.set_bias(v)),
            (LEVEL, RtValue::F32(v)) => applied(v.clamp(0.0, 1.0), |v| d.set_level(v)),
            (AUTO_LEVEL, RtValue::Bool(on)) => {
                d.set_auto_level(on);
                value
            }
            (MIX, RtValue::F32(v)) => applied(v.clamp(0.0, 1.0), |v| d.set_mix(v)),
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

/// Hands `value` to `set` and returns it as the control now holds it.
fn applied(value: f32, set: impl FnOnce(f32)) -> RtValue {
    set(value);
    RtValue::F32(value)
}
