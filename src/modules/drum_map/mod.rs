//! Drum map: kick, snare and hat triggers from a navigable map of grooves.
//!
//! The map is a 3 x 3 grid of one-bar patterns we authored (see
//! `scripts/generate_drum_map.py`). `x` moves across groove families
//! (straight, syncopated, broken) and `y` up in energy (sparse half-time,
//! standard, driving); between nodes the neighbours blend. Each voice's
//! density picks how much of its part plays, `chaos` varies it from bar to
//! bar (repeatably, from `seed`), and `swing` delays the off-beat 16ths.
//!
//! Timing arrives through the `clock` input, one rising edge per 16th note
//! (patch a clock's `beat_x4`): the map never reads the clock or the graph.
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "clock", "type": "clock", "config": { "bpm": 96.0 } },
//!     { "id": "drums", "type": "drum_map", "config": { "x": 0.5, "y": 0.5, "swing": 0.3 } }
//!   ],
//!   "connections": [
//!     { "from": "clock", "from_port": "beat_x4", "to": "drums", "to_port": "clock" }
//!   ]
//! }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::{
    CHAOS, HAT_DENSITY, KICK_DENSITY, MODE, RESOLUTION, SEED, SNARE_DENSITY, SWING, TABLE, X, Y,
};
use self::map::{CELLS, VOICES};

mod controls;
pub mod map;
mod nodes;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "drum_map";
const INPUTS: [&str; 2] = ["clock", "reset"];
const OUTPUTS: [&str; 4] = ["kick", "snare", "hat", "accent"];
const ACCENT: usize = 3;

/// 16th steps per bar, one per clock edge.
const STEPS: u32 = 16;

/// How long a trigger stays high.
const TRIGGER_SECONDS: f32 = 0.005;

/// Factory for constructing drum maps from configuration.
pub struct DrumMapFactory;

impl ModuleFactory for DrumMapFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn input_ports(&self) -> Option<&'static [&'static str]> {
        Some(&INPUTS)
    }

    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&OUTPUTS)
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let map = DrumMap::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(map)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// Hits waiting for their sample: a step delayed by swing, or the 32nd
/// between clock edges.
#[derive(Clone, Copy, Default)]
struct Pending {
    /// Samples until it fires; `None` when nothing waits.
    countdown: Option<u32>,
    /// Voices that hit (bit per output, accent last).
    mask: u8,
}

/// Kick, snare, hat and accent triggers from the pattern map.
///
/// # Inputs
/// - `clock`: a rising edge is one 16th step; 16 make a bar
/// - `reset`: a rising edge makes the next clock edge step 0 of bar 0
///
/// # Outputs
/// - `kick`, `snare`, `hat`: 5 ms triggers of 1.0 (for `sample_kit`'s `play`)
/// - `accent`: a trigger alongside hits on the steps the map ranks highest
pub struct DrumMap {
    cells: Arc<ControlCells>,
    // Controls, applied on the thread running the map.
    x: f32,
    y: f32,
    density: [f32; VOICES],
    chaos: f32,
    swing: f32,
    seed: u32,
    fine: bool,
    euclidean: bool,
    // Timing.
    trigger_samples: u32,
    period: u32,
    since_edge: u32,
    clocked: bool,
    /// The last gap looked like a pause, so the next one is taken as is.
    remeasure: bool,
    step: u32,
    bar: u32,
    pending: [Pending; 2],
    // Trigger samples left per output, and a forced low sample when a
    // trigger restarts while still high.
    remaining: [u32; 4],
    dip: [bool; 4],
    last_clock: f32,
    last_reset: f32,
    clock: [f32; MAX_BLOCK],
    reset: [f32; MAX_BLOCK],
    outputs: [[f32; MAX_BLOCK]; 4],
}

impl DrumMap {
    /// A drum map at the default controls.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// A drum map holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut map = Self {
            cells,
            x: 0.5,
            y: 0.5,
            density: [0.5; VOICES],
            chaos: 0.0,
            swing: 0.0,
            seed: 0,
            fine: false,
            euclidean: false,
            trigger_samples: ((sample_rate as f32 * TRIGGER_SECONDS) as u32).max(1),
            // 16ths at 120 BPM until two edges measure the real step.
            period: (sample_rate / 8).max(2),
            since_edge: 0,
            clocked: false,
            remeasure: false,
            step: 0,
            bar: 0,
            pending: [Pending::default(); 2],
            remaining: [0; 4],
            dip: [false; 4],
            last_clock: 0.0,
            last_reset: 0.0,
            clock: [0.0; MAX_BLOCK],
            reset: [0.0; MAX_BLOCK],
            outputs: [[0.0; MAX_BLOCK]; 4],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = map.cells.load(index) {
                let _ = apply_declared(&mut map, index, value);
            }
        }
        map
    }

    /// The voices (and accent) that hit on 32nd step `cell` of this bar.
    fn hits(&self, cell: usize) -> u8 {
        let mut mask = 0;
        for voice in 0..VOICES {
            let density = self.density[voice];
            let hit = if self.euclidean {
                let (steps, step) = if self.fine {
                    (CELLS, cell)
                } else {
                    (CELLS / 2, cell / 2)
                };
                map::euclidean(steps, density, step).then_some(step == 0)
            } else {
                let (chaos, seed, bar) = (self.chaos, self.seed, self.bar);
                map::hit(self.x, self.y, voice, cell, density, chaos, seed, bar)
            };
            if let Some(accent) = hit {
                mask |= 1 << voice;
                if accent {
                    mask |= 1 << ACCENT;
                }
            }
        }
        mask
    }

    /// Schedules the step a clock edge starts, and its 32nd at resolution 32.
    ///
    /// An off-beat 16th is late by the swing delay; each 32nd falls half way
    /// through its swung 16th, which is the same distance from either edge.
    fn start_step(&mut self) {
        let delay = (self.swing * 0.5 * self.period as f32) as u32;
        let cell = self.step as usize * 2;
        let on_beat = self.step.is_multiple_of(2);
        self.pending[0] = Pending {
            countdown: Some(if on_beat { 0 } else { delay }),
            mask: self.hits(cell),
        };
        self.pending[1] = Pending {
            countdown: self.fine.then_some((self.period + delay) / 2),
            mask: if self.fine { self.hits(cell + 1) } else { 0 },
        };
        self.step = (self.step + 1) % STEPS;
        if self.step == 0 {
            self.bar = self.bar.wrapping_add(1);
        }
    }

    fn fire(&mut self, mask: u8) {
        for output in 0..OUTPUTS.len() {
            if mask & (1 << output) != 0 {
                self.dip[output] = self.remaining[output] > 0;
                self.remaining[output] = self.trigger_samples;
            }
        }
    }

    fn frame(&mut self, i: usize) {
        let (clock, reset) = (self.clock[i], self.reset[i]);
        if reset > 0.5 && self.last_reset <= 0.5 {
            self.step = 0;
            self.bar = 0;
        }
        if clock > 0.5 && self.last_clock <= 0.5 {
            // A gap far longer than the step is most likely the clock
            // pausing: keep the step it last ran at for this edge, and take
            // the next gap whatever it is (a real slowdown).
            let paused = self.since_edge > self.period.saturating_mul(4);
            if self.clocked && (!paused || self.remeasure) {
                self.period = self.since_edge.max(2);
            }
            self.remeasure = self.clocked && paused && !self.remeasure;
            self.clocked = true;
            self.since_edge = 0;
            // A hit the edge overtook (a sudden accelerando) is dropped
            // rather than played off the grid.
            self.start_step();
        }
        (self.last_clock, self.last_reset) = (clock, reset);
        for slot in 0..self.pending.len() {
            let pending = &mut self.pending[slot];
            match pending.countdown {
                Some(0) => {
                    pending.countdown = None;
                    let mask = pending.mask;
                    self.fire(mask);
                }
                Some(left) => pending.countdown = Some(left - 1),
                None => {}
            }
        }
        for output in 0..OUTPUTS.len() {
            let high = self.remaining[output] > 0 && !self.dip[output];
            self.outputs[output][i] = if high { 1.0 } else { 0.0 };
            self.remaining[output] = self.remaining[output].saturating_sub(1);
            self.dip[output] = false;
        }
        self.since_edge = self.since_edge.saturating_add(1);
    }
}

impl Module for DrumMap {
    fn name(&self) -> &str {
        "DrumMap"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            self.frame(i);
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
            0 => &mut self.clock,
            _ => &mut self.reset,
        }
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.outputs[index.min(OUTPUTS.len() - 1)]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "clock" => self.clock.fill(value),
            "reset" => self.reset.fill(value),
            _ => return Err(format!("Unknown input port: {port}")),
        }
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = OUTPUTS.iter().position(|output| *output == port);
        let index = index.ok_or_else(|| format!("Unknown output port: {port}"))?;
        Ok(self.outputs[index][0])
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let unit = |value: f32| value.clamp(0.0, 1.0);
        let applied = match (control, value) {
            (X, RtValue::F32(v)) => {
                self.x = unit(v);
                RtValue::F32(self.x)
            }
            (Y, RtValue::F32(v)) => {
                self.y = unit(v);
                RtValue::F32(self.y)
            }
            (KICK_DENSITY | SNARE_DENSITY | HAT_DENSITY, RtValue::F32(v)) => {
                let voice = usize::from(control.0 - KICK_DENSITY.0);
                self.density[voice] = unit(v);
                RtValue::F32(self.density[voice])
            }
            (CHAOS, RtValue::F32(v)) => {
                self.chaos = unit(v);
                RtValue::F32(self.chaos)
            }
            (SWING, RtValue::F32(v)) => {
                self.swing = unit(v);
                RtValue::F32(self.swing)
            }
            (SEED, RtValue::I32(seed)) => {
                self.seed = seed.max(0) as u32;
                RtValue::I32(self.seed as i32)
            }
            (RESOLUTION, RtValue::I32(steps)) => {
                self.fine = steps > 24;
                RtValue::I32(if self.fine { 32 } else { 16 })
            }
            (MODE, RtValue::U32(mode)) if mode < 2 => {
                self.euclidean = mode == 1;
                value
            }
            (MODE, _) => return Err(Refusal::Invalid),
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
