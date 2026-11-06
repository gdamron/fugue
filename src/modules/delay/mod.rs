//! Delay: a mono delay line with feedback, timed in seconds or in note
//! values at a tempo.
//!
//! The building block for echoes, slapback, ping-pong and tape delays,
//! chorus and flanging. What goes in comes out `time` later; `feedback`
//! sends the output back into the line for repeats, and a soft clip in the
//! loop keeps them within ±1 even past unity feedback. The tempo arrives as
//! a control or a patched `bpm` input (a clock's `bpm` output), never from
//! the invention, so a delay set to a dotted eighth follows whatever clock
//! it is patched to. Time changes never click (see [`heads`]), and `hold`
//! freezes the last division into a loop (see [`hold`]).
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "clock", "type": "clock", "config": { "bpm": 100 } },
//!     { "id": "echo", "type": "delay", "config": { "division": "dotted_8th_note", "feedback": 0.45 } }
//!   ],
//!   "connections": [
//!     { "from": "clock", "from_port": "bpm", "to": "echo", "to_port": "bpm" }
//!   ]
//! }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{DelayLine, LinearRamp};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::mixer::gain_ramp_frames;
use crate::modules::note_division;
use crate::traits::ControlMeta;
use crate::{FeedbackCut, Module, MAX_BLOCK};

use self::controls::{
    BPM, DIVISION, DIVISION_BEATS, FEEDBACK, GLIDE, HOLD, LONGEST_SECONDS, MOST_FEEDBACK, TABLE,
    TIME, TIME_MOD_DEPTH,
};
use self::frames::Write;
use self::heads::{Glide, Heads};
use self::hold::Hold;
use self::inputs::{DelayInputs, INPUTS};

mod controls;
mod frames;
mod heads;
mod hold;
#[cfg(test)]
mod hold_tests;
mod inputs;
#[cfg(test)]
mod tests;

const TYPE_ID: &str = "delay";
const MAX_TIME_KEY: ConfigKey = ConfigKey::float("max_time");
const MIN_TIME_KEY: ConfigKey = ConfigKey::float("min_time");
/// The inputs only the line's writes read, by index: `audio`.
const ABSORBED: [usize; 1] = [0];
/// How long a delay holds when its config doesn't say.
const DEFAULT_MAX_TIME: f32 = 4.0;
const OUTPUTS: [&str; 1] = ["audio"];
/// Samples the line holds past its longest delay, so a read there has the
/// neighbours a fractional read needs.
const READ_MARGIN: usize = 3;

/// Below this, the line records exactly what it is given; above it, the
/// soft clip bends the signal smoothly towards ±1.
const KNEE: f32 = 0.75;
/// How close (in samples) a time must be to a whole sample to be read as
/// one: a time of N/sr, rounded to `f32`, still delays exactly N samples.
const WHOLE_SAMPLE: f64 = 1.0e-3;

/// The line's soft clip: unchanged within ±[`KNEE`], then bending smoothly
/// (no corner at the knee) towards ±1, which only an overwhelming value
/// reaches; not-a-number records as silence. Every sample
/// the line records passes through it, so repeats past unity feedback, or
/// a loop patched around the delay, stay within ±1.
#[inline]
pub(crate) fn soft_clip(value: f32) -> f32 {
    let magnitude = value.abs();
    if magnitude <= KNEE {
        return value;
    }
    if value.is_nan() {
        return 0.0;
    }
    // Written as 1 - 1/(1 + over) so a huge or infinite value reaches 1
    // rather than overflowing to infinity over infinity.
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    (KNEE + (1.0 - KNEE) * (1.0 - 1.0 / (1.0 + over))).copysign(value)
}

/// `samples`, or the whole sample it is within [`WHOLE_SAMPLE`] of.
fn whole_or_exact(samples: f64) -> f64 {
    let whole = samples.round();
    if (samples - whole).abs() < WHOLE_SAMPLE {
        whole
    } else {
        samples
    }
}

/// `samples` rounded up to a whole sample, or down to one it is within
/// [`WHOLE_SAMPLE`] of: `max_time` in `f32` seconds holds what it says.
fn whole_or_up(samples: f64) -> f64 {
    whole_or_exact(samples).ceil()
}

/// Factory for constructing delays from configuration.
pub struct DelayFactory;

impl ModuleFactory for DelayFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[MAX_TIME_KEY, MIN_TIME_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let max_time = reader.float(&MAX_TIME_KEY)?.unwrap_or(DEFAULT_MAX_TIME);
        // Past the longest, the line holds the longest, as config numbers
        // clamp; a line that holds nothing is no delay at all.
        if max_time <= 0.0 {
            return Err(
                format!("delay config 'max_time' must be above 0 seconds, got {max_time}").into(),
            );
        }
        let max_time = max_time.min(LONGEST_SECONDS);
        let min_time = reader.float(&MIN_TIME_KEY)?.unwrap_or(0.0);
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let delay = Delay::with_cells(sample_rate, max_time, min_time, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(delay)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A mono delay line with feedback, a soft clip in the loop, click-free
/// time changes and a hold loop.
///
/// # Inputs
/// - `audio`: the signal to delay
/// - `time_mod`: adds `time_mod × time_mod_depth` seconds to the time, at
///   audio rate (for chorus, flanging, tape wow)
/// - `bpm`: a tempo in BPM; while patched it times `division` and the
///   `bpm` control is ignored. Read once per block.
///
/// # Outputs
/// - `audio`: the delayed signal alone; mix it with the dry signal outside
///
/// # Controls
/// - `time`: seconds, while `division` is `off` (clamped to `min_time` and
///   `max_time`)
/// - `division`: `off`, or a note value (`dotted_8th_note`, `1/8.`) timed
///   at the tempo, a quarter note being one beat
/// - `bpm`: the tempo for `division` (default 120)
/// - `feedback`: 0 to 1.1; past 1 the repeats self-oscillate, held within
///   ±1 by the soft clip. It glides, so it never clicks.
/// - `glide`: `crossfade` (default) or `slew` (see [`heads`])
/// - `hold`: stop recording and loop the last division (see [`hold`])
/// - `time_mod_depth`: seconds of delay per unit of `time_mod`
///
/// # Config
/// - `max_time`: the longest time in seconds (default 4; above 0, and
///   held to 60). The line is allocated once, at build, to hold it.
/// - `min_time`: the shortest time in seconds (default 0, one sample;
///   held to `max_time`). Every read, `time_mod` and the hold loop
///   included, is at least this far back.
///
/// A time of a whole number of samples is exact: with feedback at 0, input
/// within ±0.75 comes out bit for bit. Times are at least one sample.
/// Repeats from `feedback` are spaced exactly the time, and so are those
/// of a loop patched around the delay through other modules: the loop
/// runs in chunks of `min_time` (see [`crate::FeedbackCut`]), so a longer
/// `min_time` makes it cheaper.
pub struct Delay {
    sample_rate: f64,
    /// The longest delay, in samples.
    longest: f64,
    /// The shortest delay, in whole samples (at least 1), and in seconds.
    shortest: usize,
    shortest_time: f32,
    /// The writes an emit kept for its absorb, and how many (see [`frames`]).
    writes: [Option<Write>; MAX_BLOCK],
    emitted: usize,
    line: DelayLine,
    heads: Heads,
    hold: Hold,
    feedback: LinearRamp,
    // Controls, applied on the thread running the delay.
    time: f32,
    division: usize,
    bpm: f32,
    time_mod_depth: f32,
    cells: Arc<ControlCells>,
    inputs: DelayInputs,
    out: [f32; MAX_BLOCK],
    /// Whether a frame has been played. Until then a change of time or
    /// feedback (a development's starting values, a patched tempo) lands
    /// at once instead of gliding there.
    heard: bool,
}

impl Delay {
    /// A delay at `sample_rate` holding up to `max_time` seconds, with
    /// default controls.
    pub fn new(sample_rate: u32, max_time: f32) -> Self {
        Self::with_cells(
            sample_rate,
            max_time,
            0.0,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    fn with_cells(
        sample_rate: u32,
        max_time: f32,
        min_time: f32,
        cells: Arc<ControlCells>,
    ) -> Self {
        let rate = f64::from(sample_rate);
        let max_time = if max_time.is_finite() {
            max_time.clamp(0.0, LONGEST_SECONDS)
        } else {
            DEFAULT_MAX_TIME
        };
        let longest = whole_or_up(f64::from(max_time) * rate).max(1.0);
        let min_time = if min_time.is_finite() {
            min_time.clamp(0.0, max_time)
        } else {
            0.0
        };
        let shortest = whole_or_up(f64::from(min_time) * rate).clamp(1.0, longest) as usize;
        let mut delay = Self {
            sample_rate: rate,
            longest,
            shortest,
            shortest_time: min_time,
            writes: [None; MAX_BLOCK],
            emitted: 0,
            line: DelayLine::new(longest as usize + READ_MARGIN + Hold::headroom(sample_rate)),
            heads: Heads::new(sample_rate, shortest as f64, Glide::Crossfade),
            hold: Hold::new(sample_rate),
            feedback: LinearRamp::new(0.0, gain_ramp_frames(sample_rate)),
            time: 0.25,
            division: 0,
            bpm: 120.0,
            time_mod_depth: 0.005,
            cells,
            inputs: DelayInputs::new(),
            out: [0.0; MAX_BLOCK],
            heard: false,
        };
        for control in [TIME, DIVISION, BPM, FEEDBACK, GLIDE, TIME_MOD_DEPTH, HOLD] {
            if let Some(value) = delay.cells.load(control) {
                let _ = apply_declared(&mut delay, control, value);
            }
        }
        // Built, the delay starts where its controls are, not gliding there.
        delay.heads.retarget(delay.time_samples());
        delay.heads.settle();
        delay.feedback.settle();
        delay
    }

    /// The tempo: the `bpm` input's while it is patched and carries one,
    /// otherwise the control's.
    fn tempo(&self) -> f64 {
        let patched = self.inputs.bpm_connected.then_some(self.inputs.bpm[0]);
        note_division::tempo(patched, self.bpm)
    }

    /// The time the controls and tempo ask for, in samples, from the
    /// shortest to the longest. One within [`WHOLE_SAMPLE`] of a whole
    /// sample is that sample.
    fn time_samples(&self) -> f64 {
        let seconds = note_division::division_seconds(self.division, self.tempo())
            .unwrap_or(f64::from(self.time));
        whole_or_exact(seconds * self.sample_rate).clamp(self.shortest as f64, self.longest)
    }

    /// The line `offset + modulation` samples back, held to the shortest
    /// and longest times, with `behind` writes the line has yet to make
    /// (see [`frames`]).
    #[inline]
    fn read(&self, offset: f32, modulation: f32, behind: usize) -> f32 {
        let offset = (offset + modulation).clamp(self.shortest as f32, self.longest as f32);
        // Between samples a cubic can overshoot the samples either side of
        // it; whole samples are already within ±1, so they read exactly.
        self.line
            .read_fractional_within(offset, self.shortest, behind)
            .clamp(-1.0, 1.0)
    }
}

impl Module for Delay {
    fn name(&self) -> &str {
        "Delay"
    }

    fn process(&mut self, frames: usize) -> bool {
        self.run(frames);
        true
    }

    fn feedback_cut(&self) -> Option<FeedbackCut> {
        Some(FeedbackCut {
            frames: self.shortest,
            absorbed: &ABSORBED,
        })
    }

    fn emit(&mut self, frames: usize) {
        self.emit_frames(frames);
    }

    fn absorb(&mut self, frames: usize) {
        self.absorb_frames(frames);
    }

    fn inputs(&self) -> &[&str] {
        &INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.out
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.out[0]),
            _ => Err(format!("Unknown output port: {port}")),
        }
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let applied = match (control, value) {
            (TIME, RtValue::F32(time)) => {
                let longest = (self.longest / self.sample_rate) as f32;
                self.time = time.clamp(self.shortest_time.min(longest), longest);
                RtValue::F32(self.time)
            }
            (BPM, RtValue::F32(bpm)) => {
                self.bpm = bpm.clamp(1.0, 1000.0);
                RtValue::F32(self.bpm)
            }
            (FEEDBACK, RtValue::F32(feedback)) => {
                let feedback = feedback.clamp(0.0, MOST_FEEDBACK);
                self.feedback.set_target(feedback);
                RtValue::F32(feedback)
            }
            (TIME_MOD_DEPTH, RtValue::F32(depth)) => {
                self.time_mod_depth = depth.clamp(0.0, 1.0);
                RtValue::F32(self.time_mod_depth)
            }
            (DIVISION, RtValue::U32(position)) if (position as usize) < DIVISION_BEATS.len() => {
                self.division = position as usize;
                value
            }
            (GLIDE, RtValue::U32(position)) => {
                let glide = Glide::from_position(position).ok_or(Refusal::Unsupported)?;
                self.heads.set_glide(glide);
                value
            }
            (HOLD, RtValue::Bool(hold)) => {
                if hold {
                    let length = self.time_samples().round() as usize;
                    self.hold.engage(length, &self.line);
                } else {
                    self.hold.release();
                }
                value
            }
            _ => return Err(Refusal::Unsupported),
        };
        self.heads.retarget(self.time_samples());
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
