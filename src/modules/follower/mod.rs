//! Follower: an envelope follower, stereo-linked, with a sidechain.
//!
//! It rectifies what it follows and smooths it with one pole rising
//! (`attack`) and another falling (`release`), so `level` traces the
//! signal's peak loudness as a linear amplitude. Patched into a
//! `gain_curve`, and that into a `gain`'s `decibels_mod`, it is the
//! detector of a compressor; with `sidechain_mix` at 1 it follows a key
//! signal instead (a kick, to duck a pad under it).
//!
//! ```json
//! { "id": "detector", "type": "follower", "config": { "attack": 0.005, "release": 0.2 } }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{LinearRamp, Svf};
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

const TYPE_ID: &str = "follower";
const INPUTS: [&str; 3] = ["audio_left", "audio_right", "sidechain"];
const OUTPUTS: [&str; 1] = ["level"];
/// The high-pass's resonance: the flattest response without a peak.
const HIGHPASS_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// How long a change of `attack`, `release` or `highpass` glides, in
/// seconds: switching the high-pass in or out crossfades over it.
const SETTING_GLIDE: f32 = 0.02;
/// An envelope smaller than this is silence: flushed to zero, so a long
/// release does not leave it on slow subnormal numbers.
const SUBNORMAL_FLUSH: f32 = 1.0e-20;

/// Factory for constructing [`Follower`] modules. Every control is also a
/// config key.
pub struct FollowerFactory;

impl ModuleFactory for FollowerFactory {
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
        let follower = Follower::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(follower)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// An envelope follower: a stereo-linked peak detector with a sidechain.
///
/// # Inputs
/// - `audio_left`, `audio_right`: followed together, the louder side at
///   each sample (a mono source can use `audio_left` alone)
/// - `sidechain`: a key signal, followed instead as `sidechain_mix` rises
///
/// # Outputs
/// - `level`: the envelope, a linear amplitude (a steady signal settles at
///   its peak magnitude)
///
/// # Controls
/// - `attack`: seconds for a rise to cover 63% of a step, 0 to 1 (default
///   0.01; 0 follows at once). A change glides over 20 ms
/// - `release`: seconds for a fall to cover 63% of a step, 0.001 to 5
///   (default 0.1). A change glides over 20 ms
/// - `sidechain_mix`: 0 follows the audio, 1 the sidechain (default 0). A
///   change glides over about 2.7 ms
/// - `highpass`: a high-pass on all three inputs, 0 to 2000 Hz, so lows
///   move the envelope less (default 0, off; a cutoff below 5 Hz runs at 5).
///   Switching it in or out crossfades, and a new cutoff glides, over
///   20 ms
pub struct Follower {
    cells: Arc<ControlCells>,
    sample_rate: f32,
    /// The share of the distance to a louder input covered each sample.
    rise: LinearRamp,
    /// The share of the distance to a quieter input covered each sample.
    fall: LinearRamp,
    mix: LinearRamp,
    /// How much of what is followed is high-passed: 0 or 1, crossfading.
    highpass_wet: LinearRamp,
    /// The high-pass cutoff, prewarped, gliding between settings.
    highpass_warp: LinearRamp,
    filters: [Svf; 3],
    envelope: f32,
    inputs: [[f32; MAX_BLOCK]; 3],
    output: [f32; MAX_BLOCK],
}

impl Follower {
    /// A follower at `sample_rate` with every control at its default.
    pub fn new(sample_rate: u32) -> Self {
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        Self::with_cells(sample_rate, cells)
    }

    /// A follower holding what `cells` hold, settled there.
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let filter = Svf::new(sample_rate, MAX_HIGHPASS, HIGHPASS_Q);
        let glide = (SETTING_GLIDE * sample_rate as f32).round() as u32;
        let mut follower = Self {
            cells,
            sample_rate: sample_rate.max(1) as f32,
            rise: LinearRamp::new(1.0, glide),
            fall: LinearRamp::new(1.0, glide),
            mix: LinearRamp::new(0.0, control_glide_frames(sample_rate)),
            highpass_wet: LinearRamp::new(0.0, glide),
            highpass_warp: LinearRamp::new(filter.prewarp(MAX_HIGHPASS), glide),
            filters: [filter, filter, filter],
            envelope: 0.0,
            inputs: [[0.0; MAX_BLOCK]; 3],
            output: [0.0; MAX_BLOCK],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = follower.cells.load(index) {
                let _ = apply_declared(&mut follower, index, value);
            }
        }
        for ramp in [
            &mut follower.rise,
            &mut follower.fall,
            &mut follower.mix,
            &mut follower.highpass_wet,
            &mut follower.highpass_warp,
        ] {
            ramp.settle();
        }
        let warp = follower.highpass_warp.value();
        for filter in &mut follower.filters {
            filter.set_prewarped(warp, HIGHPASS_Q);
        }
        follower
    }

    /// The one-pole coefficient that covers 63% of a step in `seconds`
    /// (1, following at once, for no time).
    fn coefficient(&self, seconds: f32) -> f32 {
        let frames = seconds * self.sample_rate;
        if frames > 0.0 {
            1.0 - (-1.0 / frames).exp()
        } else {
            1.0
        }
    }
}

/// A sample's magnitude, silence when it is not a number.
#[inline]
fn magnitude(x: f32) -> f32 {
    if x.is_finite() {
        x.abs()
    } else {
        0.0
    }
}

impl Module for Follower {
    fn name(&self) -> &str {
        "Follower"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        let clean = |x: f32| if x.is_finite() { x } else { 0.0 };
        for n in 0..frames {
            let mut signals = [0, 1, 2].map(|port| clean(self.inputs[port][n]));
            let filtering = self.highpass_wet.is_gliding() || self.highpass_wet.value() > 0.0;
            if filtering {
                let moving = self.highpass_warp.is_gliding();
                let warp = self.highpass_warp.next_value();
                let wet = self.highpass_wet.next_value();
                for (x, filter) in signals.iter_mut().zip(&mut self.filters) {
                    if moving {
                        filter.set_prewarped(warp, HIGHPASS_Q);
                    }
                    *x += wet * (filter.process(*x).highpass - *x);
                }
            }
            let [l, r, k] = signals;
            let audio = magnitude(l).max(magnitude(r));
            let mix = self.mix.next_value();
            let target = audio + mix * (magnitude(k) - audio);
            let (rise, fall) = (self.rise.next_value(), self.fall.next_value());
            let share = if target > self.envelope { rise } else { fall };
            self.envelope += share * (target - self.envelope);
            if self.envelope < SUBNORMAL_FLUSH {
                self.envelope = 0.0;
            }
            self.output[n] = self.envelope;
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
        &mut self.inputs[index.min(2)]
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.output
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS.iter().position(|p| *p == port);
        let index = index.ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.inputs[index].fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "level" => Ok(self.output[0]),
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
            ATTACK => {
                let attack = v.clamp(0.0, MAX_ATTACK);
                self.rise.set_target(self.coefficient(attack));
                attack
            }
            RELEASE => {
                let release = v.clamp(MIN_RELEASE, MAX_RELEASE);
                self.fall.set_target(self.coefficient(release));
                release
            }
            SIDECHAIN_MIX => {
                let mix = v.clamp(0.0, 1.0);
                self.mix.set_target(mix);
                mix
            }
            HIGHPASS => {
                let cutoff = v.clamp(0.0, MAX_HIGHPASS);
                if cutoff <= 0.0 {
                    self.highpass_wet.set_target(0.0);
                    return Ok(RtValue::F32(0.0));
                }
                let warp = self.filters[0].prewarp(cutoff);
                let idle = !self.highpass_wet.is_gliding() && self.highpass_wet.value() == 0.0;
                if idle {
                    // Out of use: start it clean, at the new cutoff.
                    self.highpass_warp.snap(warp);
                    for filter in &mut self.filters {
                        filter.reset();
                        filter.set_prewarped(warp, HIGHPASS_Q);
                    }
                } else {
                    self.highpass_warp.set_target(warp);
                }
                self.highpass_wet.set_target(1.0);
                cutoff
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
