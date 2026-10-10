use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::ClockControls;

mod controls;
mod inputs;
mod outputs;

/// Factory for constructing Clock modules from configuration.
pub struct ClockFactory;

const TYPE_ID: &str = "clock";
const BPM: ConfigKey = ConfigKey::float("bpm");
const GATE_LENGTH: ConfigKey = ConfigKey::float("gate_length");

impl ModuleFactory for ClockFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[BPM, GATE_LENGTH] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let bpm = reader.float(&BPM)?.map_or(120.0, f64::from);
        let gate_length = reader.float(&GATE_LENGTH)?.map_or(0.25, f64::from);

        let controls = ClockControls::new_with_gate_length(bpm, gate_length);
        let clock = Clock::new(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(clock)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
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
pub struct Clock {
    sample_rate: u32,
    ctrl: ClockControls,
    sample_count: u64,
    // Tempo epoch: the (sample, beats) anchor the continuous beat count is
    // measured from. Re-anchored on every bpm change to keep phase continuous.
    epoch_sample: u64,
    epoch_beats: f64,
    last_bpm: f64,
    // Timing state derived from the continuous beat count.
    beats: f64,
    phase: f32,
    // Cached output for modular routing
    outputs: outputs::ClockOutputs,
}

impl Clock {
    /// Creates a new clock with the given sample rate and controls.
    pub fn new(sample_rate: u32, controls: ClockControls) -> Self {
        let bpm = controls.bpm();
        let mut clock = Self {
            sample_rate,
            ctrl: controls,
            sample_count: 0,
            epoch_sample: 0,
            epoch_beats: 0.0,
            last_bpm: bpm,
            beats: 0.0,
            phase: 0.0,
            outputs: outputs::ClockOutputs::new(),
        };
        clock.update_signal();
        clock.update_cached_outputs(0);
        clock
    }

    fn update_signal(&mut self) {
        let bpm = self.ctrl.bpm();
        // Re-anchor the epoch on a tempo change so the beat count stays
        // continuous: beats accrued so far are preserved, and the new tempo
        // takes effect from the previous sample forward.
        if bpm != self.last_bpm {
            self.epoch_beats = self.beats;
            self.epoch_sample = self.sample_count.saturating_sub(1);
            self.last_bpm = bpm;
        }

        let samples_per_beat = self.ctrl.samples_per_beat(self.sample_rate);
        let elapsed = self.sample_count.saturating_sub(self.epoch_sample) as f64;
        let beats = self.epoch_beats + elapsed / samples_per_beat;

        self.beats = beats;
        self.phase = beats.fract() as f32;
    }

    fn update_cached_outputs(&mut self, i: usize) {
        let gate_length = self.ctrl.gate_length();
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

    /// Returns the total number of beats elapsed since the clock started.
    ///
    /// This is the continuous beat count: across a tempo change it advances
    /// smoothly rather than jumping, so it stays musically meaningful when the
    /// clock's `bpm` is automated.
    pub fn beats_elapsed(&self) -> f64 {
        self.beats
    }

    /// Returns a reference to the controls.
    pub fn controls(&self) -> &ClockControls {
        &self.ctrl
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

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("bpm", "Tempo in beats per minute")
                .with_range(1.0, 300.0)
                .with_default(120.0),
            ControlMeta::new("gate_length", "Gate length as a fraction of the pulse")
                .with_range(0.0, 1.0)
                .with_default(0.25),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "bpm" => Ok(self.ctrl.bpm() as f32),
            "gate_length" => Ok(self.ctrl.gate_length() as f32),
            _ => Err(format!("Unknown control key: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "bpm" => {
                self.ctrl.set_bpm(value as f64);
                Ok(())
            }
            "gate_length" => {
                self.ctrl.set_gate_length(value as f64);
                Ok(())
            }
            _ => Err(format!("Unknown control key: {}", key)),
        }
    }
}
