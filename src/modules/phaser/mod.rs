//! Phaser: chains of first-order allpass stages whose corners sweep,
//! mixed with the dry signal so the stages' phase cuts moving notches.
//!
//! Each side runs up to 12 stages (see [`chain`]) swept by one sine LFO,
//! the right side `stereo_phase` cycles ahead of the left, with feedback
//! around the chain solved within the sample. The sweep runs at `rate`, or
//! in time with a tempo (the `bpm` control or a patched `bpm` input) as a
//! note value. Every control glides and a change of `stage_count`
//! crossfades, so nothing clicks; the wet path ends in a soft clip, so the
//! output stays within ±1 whatever the feedback.
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "clock", "type": "clock", "config": { "bpm": 96 } },
//!     { "id": "phaser", "type": "phaser", "config": { "division": "half_note", "stage_count": 8 } }
//!   ],
//!   "connections": [
//!     { "from": "clock", "from_port": "bpm", "to": "phaser", "to_port": "bpm" }
//!   ]
//! }
//! ```
//!
//! Provenance: original, from standard techniques.

use std::f32::consts::PI;
use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{exp2, sine, tan_quarter_turn, FirstOrderAllpass, LinearRamp};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::ConfigKey;
use crate::modules::delay::soft_clip;
use crate::modules::note_division;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::chain::Chain;
use self::controls::*;

mod chain;
mod controls;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "phaser";
const INPUTS: [&str; 3] = ["audio_left", "audio_right", "bpm"];
const OUTPUTS: [&str; 2] = ["audio_left", "audio_right"];
const RIGHT: usize = 1;
const BPM_INPUT: usize = 2;
/// How long a control change glides, and a stage count crossfades, in
/// seconds.
const GLIDE: f32 = 0.02;
/// Samples between coefficient updates; the coefficient glides between.
const UPDATE: usize = 16;
/// Octaves the sweep reaches either side of `frequency` at full depth.
const REACH_OCTAVES: f32 = 3.0;
/// The lowest and highest corner, in Hz (the highest also below 0.45 of
/// the sample rate).
const LOWEST_CORNER: f32 = 20.0;
const HIGHEST_CORNER: f32 = 18_000.0;

/// Factory for constructing phasers. Every control is also a config key.
pub struct PhaserFactory;

impl ModuleFactory for PhaserFactory {
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
        let phaser = Phaser::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(phaser)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A stereo phaser. See the [module docs](self).
///
/// # Inputs
/// - `audio_left`, `audio_right`: the signal; the right copies the left
///   while unpatched, so a mono source can feed `audio_left` alone
/// - `bpm`: a tempo in BPM; while patched it times `division` and the
///   `bpm` control is ignored. Read once per block.
///
/// # Outputs
/// - `audio_left`, `audio_right`
///
/// # Controls
/// - `stage_count`: allpass stages per side, 4 to 12 (default 6)
/// - `rate`: sweep speed in Hz, 0.01 to 10, while `division` is `off`
/// - `division`, `bpm`: the sweep as a note value at a tempo, as the `lfo`
/// - `depth`: 0 to 1; the corners move between `frequency`·2^(±3·depth)
/// - `frequency`: the sweep's centre, 50 to 4000 Hz
/// - `feedback`: −0.95 to 0.95; the chain's input is scaled by
///   √(1 − feedback²), so the wet level holds as feedback rises
/// - `stereo_phase`: cycles the right sweep runs ahead of the left
/// - `mix`: output = (1 − mix)·dry + mix·wet
pub struct Phaser {
    sample_rate: f32,
    cells: Arc<ControlCells>,
    left: Chain,
    right: Chain,
    /// Each tap's weight (after stage n + 1), gliding through a crossfade.
    weights: [LinearRamp; MOST_STAGES],
    /// Samples left in a stage-count crossfade, and the stages it reaches.
    fading: u32,
    reach: usize,
    stage_count: usize,
    /// The sweep's phase in cycles, and its speed.
    phase: f64,
    rate: f32,
    division: usize,
    bpm: f32,
    /// Hz of this block's sweep.
    speed: f64,
    /// Samples until the next coefficient update.
    countdown: usize,
    // Glided once per update.
    depth: LinearRamp,
    frequency: LinearRamp,
    stereo_phase: LinearRamp,
    // Glided per sample.
    feedback: LinearRamp,
    mix: LinearRamp,
    audio: [[f32; MAX_BLOCK]; 2],
    bpm_input: [f32; MAX_BLOCK],
    right_connected: bool,
    bpm_connected: bool,
    out: [[f32; MAX_BLOCK]; 2],
}

impl Phaser {
    /// A phaser at `sample_rate` with every control at its default.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let frames = (GLIDE * sample_rate as f32).round().max(1.0) as u32;
        let updates = (frames / UPDATE as u32).max(1);
        let mut phaser = Self {
            sample_rate: sample_rate as f32,
            cells,
            left: Chain::new(),
            right: Chain::new(),
            weights: [LinearRamp::new(0.0, frames); MOST_STAGES],
            fading: 0,
            reach: MOST_STAGES,
            stage_count: 6,
            phase: 0.0,
            rate: 0.5,
            division: 0,
            bpm: 120.0,
            speed: 0.5,
            countdown: 0,
            depth: LinearRamp::new(1.0, updates),
            frequency: LinearRamp::new(1_000.0, updates),
            stereo_phase: LinearRamp::new(0.1, updates),
            feedback: LinearRamp::new(0.4, frames),
            mix: LinearRamp::new(0.5, frames),
            audio: [[0.0; MAX_BLOCK]; 2],
            bpm_input: [0.0; MAX_BLOCK],
            right_connected: false,
            bpm_connected: false,
            out: [[0.0; MAX_BLOCK]; 2],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = phaser.cells.load(index) {
                let _ = apply_declared(&mut phaser, index, value);
            }
        }
        // Built, the phaser starts where its controls are.
        for ramp in [
            &mut phaser.depth,
            &mut phaser.frequency,
            &mut phaser.stereo_phase,
            &mut phaser.feedback,
            &mut phaser.mix,
        ] {
            ramp.settle();
        }
        for weight in &mut phaser.weights {
            weight.settle();
        }
        phaser.fading = 0;
        phaser.reach = phaser.stage_count;
        phaser.update(0);
        phaser
    }

    /// Moves the sweep on one update and sets each side's coefficient to
    /// glide there over `frames` samples (0 jumps).
    fn update(&mut self, frames: usize) {
        self.phase += self.speed * frames as f64 / f64::from(self.sample_rate);
        self.phase -= self.phase.floor();
        let depth = self.depth.next_value();
        let centre = self.frequency.next_value();
        let ahead = self.stereo_phase.next_value();
        let highest = HIGHEST_CORNER
            .min(0.45 * self.sample_rate)
            .max(LOWEST_CORNER);
        let coefficient = |phase: f32| {
            let corner = centre * exp2(REACH_OCTAVES * depth * sine(phase));
            let corner = corner.clamp(LOWEST_CORNER, highest);
            FirstOrderAllpass::coefficient(tan_quarter_turn(PI * corner / self.sample_rate))
        };
        let phase = self.phase as f32;
        let (left, right) = (coefficient(phase), coefficient(phase + ahead));
        self.left.retarget(left, frames);
        self.right.retarget(right, frames);
    }

    /// The tap weights for this sample, gliding while a crossfade runs.
    #[inline]
    fn weights(&mut self, weights: &mut [f32; MOST_STAGES]) {
        if self.fading == 0 {
            return;
        }
        self.fading -= 1;
        for (weight, ramp) in weights.iter_mut().zip(&mut self.weights) {
            *weight = ramp.next_value();
        }
        if self.fading == 0 {
            self.reach = self.stage_count;
        }
    }
}

impl Module for Phaser {
    fn name(&self) -> &str {
        "Phaser"
    }

    fn process(&mut self, frames: usize) -> bool {
        let patched = self.bpm_connected.then_some(self.bpm_input[0]);
        let tempo = note_division::tempo(patched, self.bpm);
        self.speed = note_division::division_seconds(self.division, tempo)
            .map_or(f64::from(self.rate), |seconds| 1.0 / seconds);
        let mut weights = [0.0; MOST_STAGES];
        for (weight, ramp) in weights.iter_mut().zip(&self.weights) {
            *weight = ramp.value();
        }
        let source = if self.right_connected { RIGHT } else { 0 };
        for i in 0..frames {
            if self.countdown == 0 {
                self.update(UPDATE);
                self.countdown = UPDATE;
            }
            self.countdown -= 1;
            self.weights(&mut weights);
            let feedback = self.feedback.next_value();
            let scale = (1.0 - feedback * feedback).sqrt();
            let mix = self.mix.next_value();
            let reach = self.reach;
            let (dry_left, dry_right) = (self.audio[0][i], self.audio[source][i]);
            let wet = self.left.tick(dry_left, scale, feedback, &weights, reach);
            self.out[0][i] = dry_left + mix * (soft_clip(wet) - dry_left);
            let wet = self.right.tick(dry_right, scale, feedback, &weights, reach);
            self.out[1][i] = dry_right + mix * (soft_clip(wet) - dry_right);
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
            0 | RIGHT => &mut self.audio[index],
            _ => &mut self.bpm_input,
        }
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.out[index.min(1)]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.input_block_mut(index).fill(value);
        self.set_input_connected(index, true);
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
        match index {
            RIGHT => self.right_connected = connected,
            BPM_INPUT => self.bpm_connected = connected,
            _ => {}
        }
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let float = |value: RtValue, min: f32, max: f32| match value {
            RtValue::F32(v) => Ok(v.clamp(min, max)),
            _ => Err(Refusal::Unsupported),
        };
        let applied = match control {
            STAGE_COUNT => {
                let RtValue::I32(count) = value else {
                    return Err(Refusal::Unsupported);
                };
                let count = count.clamp(FEWEST_STAGES, MOST_STAGES as i32);
                self.stage_count = count as usize;
                for (n, weight) in self.weights.iter_mut().enumerate() {
                    weight.set_target(if n + 1 == self.stage_count { 1.0 } else { 0.0 });
                }
                self.fading = (GLIDE * self.sample_rate).round().max(1.0) as u32;
                self.reach = MOST_STAGES;
                RtValue::I32(count)
            }
            RATE => {
                self.rate = float(value, 0.01, 10.0)?;
                RtValue::F32(self.rate)
            }
            DIVISION => match value {
                RtValue::U32(position)
                    if (position as usize) < note_division::DIVISION_BEATS.len() =>
                {
                    self.division = position as usize;
                    value
                }
                _ => return Err(Refusal::Unsupported),
            },
            BPM => {
                self.bpm = float(value, 1.0, 1000.0)?;
                RtValue::F32(self.bpm)
            }
            _ => {
                let (ramp, min, max) = match control {
                    DEPTH => (&mut self.depth, 0.0, 1.0),
                    FREQUENCY => (&mut self.frequency, 50.0, 4000.0),
                    FEEDBACK => (&mut self.feedback, -MOST_FEEDBACK, MOST_FEEDBACK),
                    STEREO_PHASE => (&mut self.stereo_phase, 0.0, 1.0),
                    MIX => (&mut self.mix, 0.0, 1.0),
                    _ => return Err(Refusal::Unsupported),
                };
                let target = float(value, min, max)?;
                if control == STEREO_PHASE {
                    // The short way round: 0.9 to 0.1 glides forward 0.2.
                    let from = ramp.value();
                    let turn = target - from;
                    ramp.set_target(from + turn - (turn + 0.5).floor());
                } else {
                    ramp.set_target(target);
                }
                RtValue::F32(target)
            }
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
