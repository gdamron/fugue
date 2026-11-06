//! Noise source: white and pink noise at a set level.
//!
//! White noise has equal power at every frequency and sounds bright: the
//! body of a hi-hat or snare, a breath, a burst for exciting a resonator.
//! Pink noise has equal power in every octave and sounds even and soft:
//! surf, wind, rain, and a test signal that sounds level across the range.

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{LinearRamp, PinkFilter, Rng};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::mixer::gain_ramp_frames;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::{LEVEL, TABLE};

mod controls;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "noise";
const SEED_KEY: ConfigKey = ConfigKey::seed("seed");
const LEVEL_KEY: ConfigKey = ConfigKey::float("level");

/// Output port names, in `output_block` order.
const OUTPUTS: [&str; 2] = ["white", "pink"];

/// Factory for constructing noise modules from configuration.
pub struct NoiseFactory;

impl ModuleFactory for NoiseFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[SEED_KEY, LEVEL_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let seed = reader.int::<u64>(&SEED_KEY)?;
        let level = reader.float(&LEVEL_KEY)?.unwrap_or(1.0);
        let noise = Noise::new(sample_rate, seed, level);
        let surface = DeclaredSurface::new(TABLE.clone(), noise.cells.clone());
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(noise)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// White and pink noise from one seeded stream. The pink output is the
/// white one filtered, so the two are related: for two independent noise
/// layers, use two noise modules.
///
/// # Outputs
/// - `white`: uniform white noise, ±`level` (RMS `level/√3`)
/// - `pink`: the same noise shaped to −3 dB/oct (RMS about `0.18 × level`)
///
/// # Controls
/// - `level`: output level, 0 to 1; a write glides, so it never clicks
///
/// # Config
/// - `seed`: fixes the stream, so a render repeats bit for bit. Without
///   one, each module draws its own seed, so two noise sources are
///   independent rather than copies of each other.
pub struct Noise {
    rng: Rng,
    pink: PinkFilter,
    level: LinearRamp,
    cells: Arc<ControlCells>,
    white_out: [f32; MAX_BLOCK],
    pink_out: [f32; MAX_BLOCK],
}

impl Noise {
    /// A noise source at `sample_rate`, its stream fixed by `seed` (or
    /// drawn from entropy), at `level` (clamped to 0 to 1).
    pub fn new(sample_rate: u32, seed: Option<u64>, level: f32) -> Self {
        // Entropy, or the invention seed's stream for this module.
        let seed = seed.unwrap_or_else(crate::invention::seeding::entropy);
        let mut noise = Self {
            rng: Rng::new(seed),
            pink: PinkFilter::new(sample_rate),
            level: LinearRamp::new(1.0, gain_ramp_frames(sample_rate)),
            cells: Arc::new(ControlCells::new(controls::defaults())),
            white_out: [0.0; MAX_BLOCK],
            pink_out: [0.0; MAX_BLOCK],
        };
        let _ = apply_declared(&mut noise, LEVEL, RtValue::F32(level));
        // The configured level is where it starts, not a glide.
        noise.level.settle();
        noise
    }
}

impl Module for Noise {
    fn name(&self) -> &str {
        "Noise"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let level = self.level.next_value();
            let white = self.rng.bipolar();
            // The filter runs at full scale whatever the level, so a fade
            // in starts from noise already shaped, not from silence.
            let pink = self.pink.process(white);
            self.white_out[i] = white * level;
            self.pink_out[i] = pink * level;
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        // Noise has no inputs.
        &mut []
    }

    fn output_block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.white_out,
            _ => &self.pink_out,
        }
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("Unknown input port: {port}"))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "white" => Ok(self.white_out[0]),
            "pink" => Ok(self.pink_out[0]),
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
            (LEVEL, RtValue::F32(level)) => {
                let level = level.clamp(0.0, 1.0);
                self.level.set_target(level);
                Ok(RtValue::F32(level))
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
