use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::ClockControls;

mod controls;
mod inputs;
mod outputs;

/// Factory for constructing Clock modules from configuration.
pub struct ClockFactory;

impl ModuleFactory for ClockFactory {
    fn type_id(&self) -> &'static str {
        "clock"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let bpm = config.get("bpm").and_then(|v| v.as_f64()).unwrap_or(120.0);
        let gate_duration = config
            .get("gate_duration")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.25);

        let controls = ClockControls::new_with_gate_duration(bpm, gate_duration);
        let mut clock = Clock::new(sample_rate, controls.clone());

        // Apply time signature if specified
        if let Some(ts) = config.get("time_signature") {
            if let Some(beats) = ts.get("beats_per_measure").and_then(|v| v.as_u64()) {
                clock = clock.with_time_signature(beats as u32);
            }
        }

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(clock)),
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
/// Outputs timing information via the `gate` port for triggering downstream modules.
pub struct Clock {
    sample_rate: u32,
    ctrl: ClockControls,
    sample_count: u64,
    beats_per_measure: u32,
    // Timing state (previously in ClockSignal)
    beats: f64,
    phase: f32,
    measure: u64,
    beat_in_measure: u32,
    // Cached output for modular routing
    outputs: outputs::ClockOutputs,
    last_processed_sample: u64, // For pull-based processing
}

impl Clock {
    /// Creates a new clock with the given sample rate and controls.
    ///
    /// Defaults to 4 beats per measure (4/4 time).
    pub fn new(sample_rate: u32, controls: ClockControls) -> Self {
        let mut clock = Self {
            sample_rate,
            ctrl: controls,
            sample_count: 0,
            beats_per_measure: 4,
            beats: 0.0,
            phase: 0.0,
            measure: 0,
            beat_in_measure: 0,
            outputs: outputs::ClockOutputs::new(),
            last_processed_sample: 0,
        };
        clock.update_signal();
        clock.update_cached_outputs();
        clock
    }

    /// Sets the time signature by specifying beats per measure.
    pub fn with_time_signature(mut self, beats_per_measure: u32) -> Self {
        self.beats_per_measure = beats_per_measure;
        self.update_signal();
        self.update_cached_outputs();
        self
    }

    fn update_signal(&mut self) {
        let beats = self.beats_elapsed();
        let samples_per_beat = self.ctrl.samples_per_beat(self.sample_rate);
        let phase = (self.sample_count as f64 % samples_per_beat) / samples_per_beat;
        let measure = (beats / self.beats_per_measure as f64).floor() as u64;
        let beat_in_measure = (beats % self.beats_per_measure as f64).floor() as u32;

        self.beats = beats;
        self.phase = phase as f32;
        self.measure = measure;
        self.beat_in_measure = beat_in_measure;
    }

    fn update_cached_outputs(&mut self) {
        let samples_per_beat = self.ctrl.samples_per_beat(self.sample_rate);
        let sample_in_beat = self.sample_count % (samples_per_beat as u64);
        let gate_duration = self.ctrl.gate_duration();
        let gate_samples = (samples_per_beat * gate_duration) as u64;

        // Gate: PWM signal - HIGH for gate_duration% of each beat
        self.outputs.set_gate(if sample_in_beat < gate_samples {
            1.0
        } else {
            0.0
        });
    }

    /// Advances the clock by one sample.
    pub fn tick(&mut self) {
        self.sample_count += 1;
        self.update_signal();
        self.update_cached_outputs();
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
    pub fn beats_elapsed(&self) -> f64 {
        self.sample_count as f64 / self.ctrl.samples_per_beat(self.sample_rate)
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

    fn process(&mut self) -> bool {
        self.tick();
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        inputs::ClockInputs::set(port)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("bpm", "Tempo in beats per minute")
                .with_range(20.0, 300.0)
                .with_default(120.0),
            ControlMeta::new("gate_duration", "Gate duration as fraction of beat")
                .with_range(0.0, 1.0)
                .with_default(0.25),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "bpm" => Ok(self.ctrl.bpm() as f32),
            "gate_duration" => Ok(self.ctrl.gate_duration() as f32),
            _ => Err(format!("Unknown control key: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "bpm" => {
                self.ctrl.set_bpm(value as f64);
                Ok(())
            }
            "gate_duration" => {
                self.ctrl.set_gate_duration(value as f64);
                Ok(())
            }
            _ => Err(format!("Unknown control key: {}", key)),
        }
    }
}
