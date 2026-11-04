//! Chance: random rhythms and melodies in key, from steady to
//! unpredictable, with a loop that blends repetition and novelty.
//!
//! ```json
//! { "id": "chance", "type": "chance",
//!   "config": { "seed": 7, "bias": 0.4, "key": "D", "scale": "dorian", "repetition": 0.6 } }
//! ```
//!
//! Each rising edge on `clock` is a step. The rhythm section tosses coins
//! for `gate_1` and `gate_2`; the melody section draws three values and
//! plays them as pitches in key. `repetition` decides whether a step
//! replays what the loop remembers of its position or draws anew.

use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::music::scales::{KEY_NAMES, SCALE_NAMES};
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::*;
use self::melody::{Channel, Shape};
use self::memory::Memory;
use self::rhythm::{Mode, BAR};

mod controls;
mod melody;
mod memory;
mod ports;
mod rhythm;

/// Factory for constructing [`Chance`] modules from configuration. Every
/// control is also a config key; `seed` takes any 64-bit seed there.
pub struct ChanceFactory;

const TYPE_ID: &str = "chance";
const SEED_KEY: ConfigKey = ConfigKey::seed("seed");

impl ModuleFactory for ChanceFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[SEED_KEY]
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        // A saved write of "no seed" reads as no seed.
        let unseeded =
            reader.get("seed").and_then(serde_json::Value::as_f64) == Some(f64::from(UNSEEDED));
        let seed = match unseeded {
            true => None,
            false => reader.int::<u64>(&SEED_KEY)?,
        };
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Keys that are no control are left to the registry, which refuses
        // them by name.
        apply_control_keys(&surface, config, |key| {
            key != "seed" && TABLE.resolve(key).is_some()
        })?;
        let chance = Chance::with_cells(seed, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(chance)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// Random gates and pitches, stepped by a clock.
///
/// # Inputs
/// - `clock`: each rising edge is a step
/// - `reset`: a rising edge starts the bar and the loop again from step 0
///
/// # Outputs
/// - `gate_1`, `gate_2`: the rhythm's gates, high while the clock pulse is
/// - `tick`: a gate on every step
/// - `frequency_1..3`: each melody channel's pitch in Hz. Channel 1 takes a
///   new pitch as `gate_1` fires, channel 2 as `gate_2` does, channel 3
///   every step, so a voice on a gate and its channel never bends mid-note.
/// - `value_1..3`: each channel's value, 0 to 1, for modulation
///
/// # Controls
/// See [`controls`]: `mode`, `bias`, `spread`, `value_bias`, `steps`, `key`,
/// `scale`, `low_note`, `range`, `repetition`, `loop_length`, `seed`.
pub struct Chance {
    rng: StdRng,
    /// Where a write of [`UNSEEDED`] restarts the stream from.
    entropy: u64,
    memory: Memory,
    mode: Mode,
    bias: f32,
    repetition: f32,
    loop_length: usize,
    shape: Shape,
    channels: [Channel; 3],
    /// Steps since the last reset.
    step: usize,
    gates: [bool; 3],
    last_clock: f32,
    last_reset: f32,
    cells: Arc<ControlCells>,
    inputs: ports::Inputs,
    outputs: ports::Outputs,
}

impl Chance {
    /// A chance module seeded with `seed` (from entropy without one),
    /// holding its controls in `cells` and starting from what they hold.
    fn with_cells(seed: Option<u64>, cells: Arc<ControlCells>) -> Self {
        let mut entropy: StdRng = rand::make_rng();
        let mut rng = match seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::seed_from_u64(entropy.random()),
        };
        let memory = Memory::new(&mut rng);
        let shape = Shape::new();
        let channel = Channel::new(&shape);
        let mut chance = Self {
            rng,
            entropy: entropy.random(),
            memory,
            mode: Mode::Complementary,
            bias: 0.5,
            repetition: 0.0,
            loop_length: 8,
            shape,
            channels: [channel; 3],
            step: 0,
            gates: [false; 3],
            last_clock: 0.0,
            last_reset: 0.0,
            cells,
            inputs: ports::Inputs::new(),
            outputs: ports::Outputs::new(),
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if index == SEED {
                continue;
            }
            if let Some(value) = chance.cells.load(index) {
                let _ = apply_declared(&mut chance, index, value);
            }
        }
        let shape = &chance.shape;
        let value = shape.value_bias;
        chance
            .channels
            .iter_mut()
            .for_each(|c| c.take(shape, value));
        let seed = seed.map_or(UNSEEDED, |seed| seed as f32);
        chance.cells.publish(SEED, RtValue::F32(seed));
        chance
    }

    /// Restarts the stream from `rng`, forgetting the loop.
    fn restart(&mut self, rng: StdRng) {
        self.rng = rng;
        self.memory.refill(&mut self.rng);
    }

    /// One step: draw (or replay), decide the gates, and move the
    /// channels whose gates fired.
    fn advance(&mut self) {
        let position = self.step % self.loop_length;
        let draw = self.memory.next(&mut self.rng, position, self.repetition);
        let [one, two] = rhythm::fires(self.mode, self.bias, draw.coins, self.step % BAR);
        self.gates = [one, two, true];
        let moves = self.channels.iter_mut().zip(draw.values).zip(self.gates);
        for ((channel, draw), fired) in moves {
            if fired {
                channel.take(&self.shape, self.shape.value(draw));
            }
        }
        self.step = self.step.wrapping_add(1);
    }
}

impl Module for Chance {
    fn name(&self) -> &str {
        "Chance"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let reset = self.inputs.reset(i);
            if reset > 0.5 && self.last_reset <= 0.5 {
                self.step = 0;
            }
            self.last_reset = reset;

            let clock = self.inputs.clock(i);
            if clock > 0.5 && self.last_clock <= 0.5 {
                self.advance();
            } else if clock <= 0.5 {
                self.gates = [false; 3];
            }
            self.last_clock = clock;

            let [a, b, c] = self.channels;
            self.outputs.set(
                i,
                self.gates,
                [a.frequency, b.frequency, c.frequency],
                [a.value, b.value, c.value],
            );
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &ports::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &ports::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    /// Every change takes effect from the next step, so none clicks. A
    /// seed always restarts the stream, even the seed it holds.
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let applied = match (control, value) {
            (MODE, RtValue::U32(position)) => {
                self.mode = Mode::from_position(position).ok_or(Refusal::Invalid)?;
                value
            }
            (KEY, RtValue::U32(position)) if (position as usize) < KEY_NAMES.len() => {
                self.shape.key = position as u8;
                self.shape.refresh();
                value
            }
            (SCALE, RtValue::U32(position)) if (position as usize) < SCALE_NAMES.len() => {
                self.shape.scale = position as usize;
                self.shape.refresh();
                value
            }
            (KEY | SCALE, RtValue::U32(_)) => return Err(Refusal::Invalid),
            (SEED, RtValue::F32(seed)) if seed < 0.0 => {
                self.restart(StdRng::seed_from_u64(self.entropy));
                self.entropy = self.rng.random();
                RtValue::F32(UNSEEDED)
            }
            (SEED, RtValue::F32(seed)) => {
                let seed = seed as u64;
                self.restart(StdRng::seed_from_u64(seed));
                RtValue::F32(seed as f32)
            }
            (control, RtValue::F32(number)) => RtValue::F32(self.apply_number(control, number)?),
            _ => return Err(Refusal::Invalid),
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

impl Chance {
    /// Applies a number control, returning what it holds.
    fn apply_number(&mut self, control: ControlIndex, value: f32) -> Result<f32, Refusal> {
        let unit = value.clamp(0.0, 1.0);
        let shape = &mut self.shape;
        let held = match control {
            BIAS => {
                self.bias = unit;
                unit
            }
            REPETITION => {
                self.repetition = unit;
                unit
            }
            SPREAD => {
                shape.spread = unit;
                unit
            }
            VALUE_BIAS => {
                shape.value_bias = unit;
                unit
            }
            STEPS => {
                shape.steps = unit;
                shape.refresh();
                unit
            }
            LOW_NOTE => {
                shape.low_note = value.clamp(0.0, 127.0).trunc();
                shape.low_note
            }
            RANGE => {
                shape.range = value.clamp(1.0, 48.0).trunc();
                shape.range
            }
            LOOP_LENGTH => {
                self.loop_length = (value.clamp(1.0, MAX_LOOP as f32) as usize).max(1);
                self.loop_length as f32
            }
            _ => return Err(Refusal::Unsupported),
        };
        Ok(held)
    }
}

#[cfg(test)]
mod tests;
