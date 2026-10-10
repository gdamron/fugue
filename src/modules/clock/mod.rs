use std::sync::Arc;
use std::time::Duration;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue, Timeline,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::traits::ControlMeta;
use crate::{ControlValue, Module};

use self::controls::{BPM, GATE_LENGTH, POSITION, RESET, TABLE};

mod controls;
mod inputs;
mod outputs;
mod timeline;

/// Factory for constructing Clock modules from configuration.
pub struct ClockFactory;

const TYPE_ID: &str = "clock";
const BPM_KEY: ConfigKey = ConfigKey::float("bpm");
const GATE_LENGTH_KEY: ConfigKey = ConfigKey::float("gate_length");
const RESET_ON_RELOAD_KEY: ConfigKey = ConfigKey::boolean("reset_on_reload");

impl ModuleFactory for ClockFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[BPM_KEY, GATE_LENGTH_KEY, RESET_ON_RELOAD_KEY] }
    }

    fn writes_on_reload(&self, config: &serde_json::Value) -> Vec<(&'static str, ControlValue)> {
        timeline::writes_on_reload(config)
    }

    fn reload_keys(&self) -> &'static [&'static str] {
        const { &[RESET_ON_RELOAD_KEY.key] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let bpm = reader.float(&BPM_KEY)?.map_or(120.0, f64::from);
        let gate_length = reader.float(&GATE_LENGTH_KEY)?.map_or(0.25, f64::from);
        timeline::reset_on_reload(config)?;

        let clock = Clock::with_gate_length(sample_rate, bpm, gate_length);
        let surface = DeclaredSurface::new(TABLE.clone(), clock.cells.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(clock)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A master clock that generates timing signals for tempo-synchronized modules.
///
/// Outputs a gate on every beat (`beat`), and at subdivisions and multiples
/// of it (`beat_x2`, `beat_x4`, `beat_d2`, `beat_d4`), for driving
/// downstream modules. The engine has no metre: bars are the composer's.
///
/// # Tempo changes are phase-continuous
///
/// The running beat count is anchored to a *tempo epoch* — the
/// `(sample, beats)` point at which the tempo last changed. Beats past the
/// epoch accrue at the current samples-per-beat, so when `bpm` changes (e.g.
/// a [`control_scheduler`](crate::modules::control_scheduler) writing a score
/// tempo map) the beat count, and therefore every gate's phase, is continuous
/// across the seam — no clipped or doubled pulse. With a constant tempo the
/// epoch stays at `(0, 0)` and the beat count is simply `sample_count / spb`.
///
/// # Beat position and reset
///
/// The beat count is the clock's *position*: beat 0 is its first gate, so
/// beat `N` falls on the first sample whose position reaches `N`, the sample
/// the `beat` gate rises on. The `reset` control returns it to beat 0: its
/// next sample is at position 0 exactly, so a reset every `N` beats loops
/// them `N` beats long to the sample. (A new clock's first sample is one
/// sample past 0, as it has always been.) The read-only
/// `position` control reports it. A reload keeps the position of a clock it
/// keeps, unless the clock's config sets `reset_on_reload`.
pub struct Clock {
    sample_rate: u32,
    // Controls, applied on the thread running the clock.
    bpm: f32,
    gate_length: f32,
    cells: Arc<ControlCells>,
    sample_count: u64,
    // Tempo epoch: the (sample, beats) anchor the continuous beat count is
    // measured from. Re-anchored on every bpm change to keep phase continuous.
    epoch_sample: u64,
    epoch_beats: f64,
    last_bpm: f64,
    // Timing state derived from the continuous beat count.
    beats: f64,
    phase: f32,
    // Whether it has output a sample since it was built or reset, and the
    // beats begun before the latest reset (see `Timeline::beats_before`).
    started: bool,
    beats_before: u64,
    // Cached output for modular routing
    outputs: outputs::ClockOutputs,
}

impl Clock {
    /// Creates a clock at `bpm` with gates a quarter of a pulse long.
    pub fn new(sample_rate: u32, bpm: f64) -> Self {
        Self::with_gate_length(sample_rate, bpm, 0.25)
    }

    /// Creates a clock at `bpm` with gates `gate_length` of a pulse long
    /// (clamped to 0 to 1).
    pub fn with_gate_length(sample_rate: u32, bpm: f64, gate_length: f64) -> Self {
        let mut clock = Self {
            sample_rate,
            bpm: 120.0,
            gate_length: 0.25,
            cells: Arc::new(ControlCells::new(controls::defaults())),
            sample_count: 0,
            epoch_sample: 0,
            epoch_beats: 0.0,
            last_bpm: 0.0,
            beats: 0.0,
            phase: 0.0,
            started: false,
            beats_before: 0,
            outputs: outputs::ClockOutputs::new(),
        };
        let _ = apply_declared(&mut clock, BPM, RtValue::F32(bpm as f32));
        let _ = apply_declared(&mut clock, GATE_LENGTH, RtValue::F32(gate_length as f32));
        clock.last_bpm = clock.bpm();
        clock.update_signal();
        clock.update_cached_outputs(0);
        clock
    }

    /// The tempo in beats per minute.
    pub fn bpm(&self) -> f64 {
        f64::from(self.bpm)
    }

    /// The number of samples per beat at the current tempo.
    pub fn samples_per_beat(&self) -> f64 {
        (self.sample_rate as f64 * 60.0) / self.bpm()
    }

    fn update_signal(&mut self) {
        let bpm = self.bpm();
        // Re-anchor the epoch on a tempo change so the beat count stays
        // continuous: beats accrued so far are preserved, and the new tempo
        // takes effect from the previous sample forward. Before the first
        // sample since the clock was built or reset there is nothing to
        // keep: the epoch already starts the count.
        if bpm != self.last_bpm {
            if self.started {
                self.epoch_beats = self.beats;
                self.epoch_sample = self.sample_count.saturating_sub(1);
            }
            self.last_bpm = bpm;
        }

        let samples_per_beat = self.samples_per_beat();
        let elapsed = self.sample_count.saturating_sub(self.epoch_sample) as f64;
        let beats = self.epoch_beats + elapsed / samples_per_beat;

        self.beats = beats;
        self.phase = beats.fract() as f32;
    }

    fn update_cached_outputs(&mut self, i: usize) {
        let gate_length = f64::from(self.gate_length);
        let beats = self.beats;

        // PWM gate at a subdivision of `pulses_per_beat` pulses per beat. The
        // pulse phase is the fractional position within the current pulse;
        // `gate_length` is the duty cycle, so a shorter note keeps a
        // proportional gap. Phase comes from the continuous beat count, so a
        // mid-stream tempo change never clips or doubles a pulse at the seam.
        let pwm = |pulses_per_beat: f64| -> f32 {
            let phase = (beats * pulses_per_beat).fract();
            if phase < gate_length {
                1.0
            } else {
                0.0
            }
        };

        self.outputs.set_all(
            i,
            pwm(1.0),  // beat
            pwm(0.25), // beat_d4: one pulse every 4 beats
            pwm(0.5),  // beat_d2: one pulse every 2 beats
            pwm(2.0),  // beat_x2: 2 pulses per beat
            pwm(4.0),  // beat_x4: 4 pulses per beat
        );
    }

    /// Advances the clock by one sample, storing gate outputs at frame `i`.
    fn advance(&mut self, i: usize) {
        self.sample_count += 1;
        self.update_signal();
        self.started = true;
        self.update_cached_outputs(i);
    }

    /// Advances the clock by one sample (writes gate outputs to frame 0).
    pub fn tick(&mut self) {
        self.advance(0);
    }

    /// Returns the total number of samples elapsed since the clock started.
    pub fn samples_elapsed(&self) -> u64 {
        self.sample_count
    }

    /// Returns the total time elapsed since the clock started.
    pub fn time_elapsed(&self) -> Duration {
        Duration::from_secs_f64(self.sample_count as f64 / self.sample_rate as f64)
    }

    /// Returns the beats elapsed since the clock started, or since it was
    /// last reset.
    ///
    /// This is the continuous beat count: across a tempo change it advances
    /// smoothly rather than jumping, so it stays musically meaningful when the
    /// clock's `bpm` is automated.
    pub fn beats_elapsed(&self) -> f64 {
        self.beats
    }

    /// Returns the sample rate this clock was configured with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl Module for Clock {
    fn name(&self) -> &str {
        "Clock"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            self.advance(i);
        }
        let position = self.beat_position().max(0.0) as f32;
        self.cells.publish(POSITION, RtValue::F32(position));
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        // Clock has no inputs.
        &mut []
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        inputs::ClockInputs::set(port)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (BPM, RtValue::F32(bpm)) => self.bpm = bpm,
            (GATE_LENGTH, RtValue::F32(length)) => self.gate_length = length.clamp(0.0, 1.0),
            (RESET, RtValue::Bool(fire)) => {
                if fire {
                    self.reset();
                }
                return Ok(RtValue::Bool(false));
            }
            _ => return Err(Refusal::Unsupported),
        }
        Ok(match control {
            BPM => RtValue::F32(self.bpm),
            _ => RtValue::F32(self.gate_length),
        })
    }

    #[allow(private_interfaces)]
    fn timeline(&self) -> Option<&dyn Timeline> {
        Some(self)
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
