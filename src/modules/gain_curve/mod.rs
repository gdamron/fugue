//! Gain curve: a compressor's static curve, from a level to the dB it asks
//! for.
//!
//! The curve works in the log domain (after Giannoulis, Massberg and
//! Reiss, "Digital Dynamic Range Compressor Design", 2012). Below the
//! threshold it leaves the level alone. Above it, each `ratio` dB in comes
//! out as one, and a soft knee of `knee` dB bends between the two. The
//! output is that change, 0 dB or below. Patch a `follower`'s `level` in
//! and the output into a `gain`'s `decibels_mod`, and the three are a
//! compressor.
//!
//! ```json
//! { "id": "curve", "type": "gain_curve", "config": { "threshold": -24.0, "ratio": 3.0 } }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{level_to_decibels, LinearRamp};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::ConfigKey;
use crate::modules::gain::control_glide_frames;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::*;

mod controls;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "gain_curve";
const INPUTS: [&str; 1] = ["level"];
const OUTPUTS: [&str; 1] = ["decibels"];

/// Factory for constructing [`GainCurve`] modules. Every control is also
/// a config key.
pub struct GainCurveFactory;

impl ModuleFactory for GainCurveFactory {
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
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let curve = GainCurve::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(curve)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A compressor's static curve in dB.
///
/// # Inputs
/// - `level`: a linear amplitude, its magnitude used (a `follower`'s
///   `level`); 1e-6 and below, or not a number, reads as −120 dB
///
/// # Outputs
/// - `decibels`: the gain change the curve asks for, 0 dB or below
///
/// # Controls
/// - `threshold`: where compression starts, −80 to 0 dB (default −20)
/// - `ratio`: dB over the threshold in per dB out, 1 to 50 (default 4)
/// - `knee`: the soft knee's width around the threshold, 0 to 24 dB
///   (default 6; 0 is a hard knee)
///
/// Every change glides over about 2.7 ms. The ratio glides as its
/// reciprocal, the curve's slope, so the output moves evenly.
pub struct GainCurve {
    cells: Arc<ControlCells>,
    threshold: LinearRamp,
    /// `1 / ratio - 1`: how much of each dB over the threshold is taken off.
    slope: LinearRamp,
    knee: LinearRamp,
    input: [f32; MAX_BLOCK],
    output: [f32; MAX_BLOCK],
}

impl GainCurve {
    /// A gain curve at `sample_rate` with every control at its default.
    pub fn new(sample_rate: u32) -> Self {
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        Self::with_cells(sample_rate, cells)
    }

    /// A gain curve holding what `cells` hold, settled there.
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let glide = control_glide_frames(sample_rate);
        let mut curve = Self {
            cells,
            threshold: LinearRamp::new(0.0, glide),
            slope: LinearRamp::new(0.0, glide),
            knee: LinearRamp::new(0.0, glide),
            input: [0.0; MAX_BLOCK],
            output: [0.0; MAX_BLOCK],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = curve.cells.load(index) {
                let _ = apply_declared(&mut curve, index, value);
            }
        }
        curve.threshold.settle();
        curve.slope.settle();
        curve.knee.settle();
        curve
    }
}

/// The gain change, in dB, for a level `x` dB against threshold `t`, slope
/// `s` (`1 / ratio - 1`) and knee width `w`.
#[inline]
pub(crate) fn gain_change(x: f32, t: f32, s: f32, w: f32) -> f32 {
    let over = x - t;
    if 2.0 * over < -w {
        0.0
    } else if 2.0 * over.abs() <= w && w > 0.0 {
        let into = over + 0.5 * w;
        s * into * into / (2.0 * w)
    } else {
        s * over
    }
}

impl Module for GainCurve {
    fn name(&self) -> &str {
        "Gain Curve"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        for (out, &level) in self.output[..frames].iter_mut().zip(&self.input[..frames]) {
            let (t, s) = (self.threshold.next_value(), self.slope.next_value());
            let w = self.knee.next_value();
            *out = gain_change(level_to_decibels(level), t, s, w);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.input
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.output
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "level" => {
                self.input.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {port}")),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "decibels" => Ok(self.output[0]),
            _ => Err(format!("Unknown output port: {port}")),
        }
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let RtValue::F32(v) = value else {
            return Err(Refusal::Unsupported);
        };
        let held = match control {
            THRESHOLD => {
                let threshold = v.clamp(MIN_THRESHOLD, MAX_THRESHOLD);
                self.threshold.set_target(threshold);
                threshold
            }
            RATIO => {
                let ratio = v.clamp(MIN_RATIO, MAX_RATIO);
                self.slope.set_target(1.0 / ratio - 1.0);
                ratio
            }
            KNEE => {
                let knee = v.clamp(0.0, MAX_KNEE);
                self.knee.set_target(knee);
                knee
            }
            _ => return Err(Refusal::Unsupported),
        };
        Ok(RtValue::F32(held))
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
