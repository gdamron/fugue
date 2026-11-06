//! Low Frequency Oscillator (LFO) module for modulation.
//!
//! The LFO generates sub-audio frequency waveforms used to modulate other
//! parameters like pitch (vibrato), amplitude (tremolo), or filter cutoff.
//!
//! # Features
//!
//! - Multiple waveforms: sine, triangle, square, sawtooth
//! - Rate range: 0.01 Hz to 20 Hz (typical LFO range)
//! - Bipolar output (-1.0 to +1.0) for FM/pitch modulation
//! - Unipolar output (0.0 to +1.0) for amplitude modulation
//! - Sync input to reset phase on trigger
//! - Rate modulation input for complex rhythmic effects
//! - Tempo sync: `division` times one cycle as a note value at a tempo (the
//!   `bpm` control, or a patched `bpm` input such as a clock's)
//! - `phase` offsets the cycle (two LFOs a quarter cycle apart), gliding
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "lfo", "type": "lfo", "config": { "rate": 5.0, "waveform": "sine" } },
//!     { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0, "frequency_mod_depth": 20.0 } }
//!   ],
//!   "connections": [
//!     { "from": "lfo", "from_port": "bipolar", "to": "osc", "to_port": "frequency_mod" }
//!   ]
//! }
//! ```

use std::f32::consts::PI;
use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::LinearRamp;
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::note_division;
use crate::modules::OscillatorType;
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{BPM, DIVISION, PHASE, RATE, RATE_MOD_DEPTH, TABLE, WAVEFORM};

mod controls;
mod inputs;
mod outputs;

/// Low Frequency Oscillator for modulation.
///
/// Like a slow-moving oscillator that creates rhythmic changes to other
/// parameters. In Eurorack terms, this is a modulation source that you'd
/// patch into CV inputs.
///
/// # Outputs
///
/// - `bipolar` - Bipolar signal (-1.0 to +1.0), ideal for pitch/FM modulation
/// - `unipolar` - Unipolar signal (0.0 to +1.0), ideal for amplitude modulation
///
/// # Inputs
///
/// - `sync` - Trigger input (rising edge resets phase to 0)
/// - `rate_mod` - Rate modulation (adds Hz to `rate`, scaled by `rate_mod_depth`)
/// - `bpm` - A tempo in BPM; while patched it times `division` and the `bpm`
///   control is ignored. Read once per block.
///
/// # Controls
///
/// - `rate` - LFO rate in Hz (default: 1.0)
/// - `rate_mod_depth` - Hz added per unit of `rate_mod` (default: 1.0)
/// - `waveform` - Waveform type (0=Sine, 1=Square, 2=Sawtooth, 3=Triangle)
/// - `division` - `off` (default), or one cycle as a note value at the tempo
///   (a quarter note is one beat); `rate` is then ignored
/// - `bpm` - The tempo for `division` (default 120)
/// - `phase` - Cycles added to the phase (0 to 1); a change glides
///
/// A change of rate, tempo or division changes the speed only: the phase
/// carries on, so the output never steps.
pub struct Lfo {
    phase: f32,
    sample_rate: u32,

    // Controls, applied on the thread running the LFO.
    rate: f32,
    rate_mod_depth: f32,
    waveform: OscillatorType,
    division: usize,
    bpm: f32,
    /// The `phase` offset, gliding to each new value.
    offset: LinearRamp,
    /// This block's synced rate in Hz, or `None` while `division` is off.
    synced: Option<f64>,
    /// The phase while synced, kept in `f64` so a cycle lasts its note
    /// value to the sample (an `f32` phase drifts by a few samples a cycle).
    precise: f64,
    cells: Arc<ControlCells>,

    // Input values
    inputs: inputs::LfoInputs,
    prev_sync: f32,

    // Cached outputs
    outputs: outputs::LfoOutputs,
}

impl Lfo {
    /// Creates a new LFO with default controls.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// An LFO holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut lfo = Self {
            phase: 0.0,
            sample_rate,
            rate: 1.0,
            rate_mod_depth: 1.0,
            waveform: OscillatorType::Sine,
            division: 0,
            bpm: 120.0,
            offset: LinearRamp::new(0.0, (PHASE_GLIDE * sample_rate as f32).round() as u32),
            synced: None,
            precise: 0.0,
            cells,
            inputs: inputs::LfoInputs::new(),
            prev_sync: 0.0,
            outputs: outputs::LfoOutputs::new(),
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = lfo.cells.load(index) {
                let _ = apply_declared(&mut lfo, index, value);
            }
        }
        // Built, the LFO starts at its offset rather than gliding there.
        lfo.offset.settle();
        lfo
    }

    /// Sets the waveform type (legacy API).
    pub fn with_waveform(mut self, waveform: OscillatorType) -> Self {
        self.set_waveform(waveform);
        self
    }

    /// Sets the rate in Hz (legacy API).
    pub fn with_rate(mut self, rate: f32) -> Self {
        self.set_rate(rate);
        self
    }

    /// Sets the waveform type (legacy API).
    pub fn set_waveform(&mut self, waveform: OscillatorType) {
        let _ = apply_declared(self, WAVEFORM, RtValue::U32(waveform.position()));
    }

    /// Sets the rate in Hz (legacy API).
    pub fn set_rate(&mut self, rate: f32) {
        let _ = apply_declared(self, RATE, RtValue::F32(rate));
    }

    /// Resets the phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.precise = 0.0;
    }

    /// Generates the sample for frame `i` based on current waveform and phase.
    fn generate(&mut self, i: usize) -> f32 {
        // Check for sync trigger (rising edge detection)
        if self.inputs.sync(i) > 0.5 && self.prev_sync <= 0.5 {
            self.phase = 0.0;
            self.precise = 0.0;
        }
        self.prev_sync = self.inputs.sync(i);

        let base_rate = self.rate;
        let rate_mod_depth = self.rate_mod_depth;
        let waveform = self.waveform;

        // Calculate effective rate with modulation
        let effective_freq =
            (base_rate + self.inputs.rate_mod(i) * rate_mod_depth).clamp(0.001, 100.0);

        let offset = self.offset.next_value();
        let phase = if offset == 0.0 {
            self.phase
        } else {
            let phase = self.phase + offset;
            phase - phase.floor()
        };

        // Generate waveform (bipolar: -1.0 to +1.0)
        let sample = match waveform {
            OscillatorType::Sine => (phase * 2.0 * PI).sin(),
            OscillatorType::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            OscillatorType::Sawtooth => 2.0 * phase - 1.0,
            OscillatorType::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
        };

        // Advance phase
        if let Some(hz) = self.synced {
            let rate_mod = f64::from(self.inputs.rate_mod(i) * rate_mod_depth);
            let freq = (hz + rate_mod).clamp(0.001, 100.0);
            self.precise += freq / f64::from(self.sample_rate);
            self.precise -= self.precise.floor();
            // A phase a hair below a whole cycle can round up to exactly one.
            self.phase = (self.precise as f32).min(1.0 - f32::EPSILON);
        } else {
            self.phase += effective_freq / self.sample_rate as f32;
            self.phase -= self.phase.floor();
        }

        sample
    }
}

impl Module for Lfo {
    fn name(&self) -> &str {
        "Lfo"
    }

    fn process(&mut self, frames: usize) -> bool {
        let tempo = note_division::tempo(self.inputs.bpm(), self.bpm);
        let synced = note_division::division_seconds(self.division, tempo).map(|s| 1.0 / s);
        if self.synced.is_none() && synced.is_some() {
            self.precise = f64::from(self.phase);
        }
        self.synced = synced;
        for i in 0..frames {
            let out = self.generate(i);
            self.outputs.set_bipolar(i, out);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
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
            (RATE, RtValue::F32(rate)) => {
                self.rate = rate.clamp(0.001, 100.0);
                RtValue::F32(self.rate)
            }
            (RATE_MOD_DEPTH, RtValue::F32(depth)) => {
                self.rate_mod_depth = depth.max(0.0);
                RtValue::F32(self.rate_mod_depth)
            }
            (WAVEFORM, RtValue::U32(position)) => {
                self.waveform = OscillatorType::from_position(position).ok_or(Refusal::Invalid)?;
                value
            }
            (DIVISION, RtValue::U32(position))
                if (position as usize) < note_division::DIVISION_BEATS.len() =>
            {
                self.division = position as usize;
                value
            }
            (BPM, RtValue::F32(bpm)) => {
                self.bpm = bpm.clamp(1.0, 1000.0);
                RtValue::F32(self.bpm)
            }
            (PHASE, RtValue::F32(phase)) => {
                let phase = phase.clamp(0.0, 1.0);
                // The short way round: 0.9 to 0.1 glides forward 0.2, not
                // back 0.8, and 0 to 1 does not move.
                let from = self.offset.value();
                let turn = phase - from;
                self.offset.set_target(from + turn - (turn + 0.5).floor());
                RtValue::F32(phase)
            }
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

/// Factory for constructing LFO modules from configuration.
pub struct LfoFactory;

const TYPE_ID: &str = "lfo";
/// How long a `phase` change glides, in seconds.
const PHASE_GLIDE: f32 = 0.02;
const RATE_KEY: ConfigKey = ConfigKey::float("rate");
const RATE_MOD_DEPTH_KEY: ConfigKey = ConfigKey::float("rate_mod_depth");

impl ModuleFactory for LfoFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[RATE_KEY, RATE_MOD_DEPTH_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let rate = reader.float(&RATE_KEY)?.unwrap_or(1.0);
        let rate_mod_depth = reader.float(&RATE_MOD_DEPTH_KEY)?.unwrap_or(1.0);

        let cells = Arc::new(ControlCells::new(controls::defaults()));
        cells.publish(RATE, RtValue::F32(rate));
        cells.publish(RATE_MOD_DEPTH, RtValue::F32(rate_mod_depth));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Only text names a waveform; anything else leaves the default, as
        // it always has.
        let named = config
            .get("waveform")
            .is_some_and(|value| value.is_string());
        apply_control_keys(&surface, config, |key| {
            (named && key == "waveform") || matches!(key, "division" | "bpm" | "phase")
        })?;
        let lfo = Lfo::with_cells(sample_rate, cells);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(lfo)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

#[cfg(test)]
mod tests;
