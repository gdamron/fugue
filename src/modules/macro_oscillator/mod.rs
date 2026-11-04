//! Macro oscillator: one voice, many synthesis models, played with three
//! macro controls.
//!
//! `model` picks how the sound is made; `harmonics`, `timbre` and `morph`
//! shape it in that model's own terms. A warm analog lead, a hollow
//! detuned pad or a buzzing sync sound is a model and three knobs away,
//! instead of a patch of ten modules.

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::LinearRamp;
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::ConfigKey;
use crate::modules::mixer::gain_ramp_frames;
use crate::traits::ControlMeta;
use crate::{Module, MAX_BLOCK};

use self::controls::{
    DECAY, FREQUENCY, FREQUENCY_MOD_DEPTH, HARMONICS, LEVEL, MAX_DECAY, MIN_DECAY, MODEL, MORPH,
    TABLE, TIMBRE,
};
use self::engine::ENGINE_BLOCK;
use self::engines::MODEL_COUNT;
use self::voice::{Block, Gate, Voice};

mod controls;
mod engine;
mod engines;
mod lpg;
mod oscillators;
#[cfg(test)]
mod tests;
mod voice;

const TYPE_ID: &str = "macro_oscillator";

/// Input port names, in `input_block_mut` order.
const INPUTS: [&str; 4] = ["frequency", "frequency_mod", "trigger", "level"];
const FREQUENCY_IN: usize = 0;
const FREQUENCY_MOD_IN: usize = 1;
const TRIGGER_IN: usize = 2;
const LEVEL_IN: usize = 3;

/// Output port names, in `output_block` order.
const OUTPUTS: [&str; 2] = ["audio", "aux"];

/// The gate's 60 dB decay time at 0 on its tuned scale, and how many
/// doublings of that time one unit of the scale makes (measured from the
/// envelope's response).
const DECAY_AT_ZERO_SECONDS: f32 = 0.226;
const DECAY_DOUBLINGS_PER_UNIT: f32 = 6.08;

/// Factory for constructing macro oscillators from configuration.
pub struct MacroOscillatorFactory;

impl ModuleFactory for MacroOscillatorFactory {
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
        // Every config key is a control's starting value; the registry
        // refuses any other key.
        apply_control_keys(&surface, config, |key| TABLE.resolve(key).is_some())?;
        let module = MacroOscillator::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// One voice with a choice of synthesis models.
///
/// # Inputs
/// - `frequency`: pitch in Hz; when connected it overrides the control
/// - `frequency_mod`: frequency modulation, scaled by `frequency_mod_depth`
/// - `trigger`: each rising edge pings the low-pass gate
/// - `level`: when connected, how far the low-pass gate is open, 0 to 1
///
/// With neither `trigger` nor `level` connected the voice sounds
/// continuously, with no gate.
///
/// # Outputs
/// - `audio`: the model's main sound
/// - `aux`: the model's variant sound
///
/// # Controls
/// - `model`: the synthesis model; a change crossfades over 20 ms
/// - `frequency`, `frequency_mod_depth`: pitch and FM depth in Hz
/// - `harmonics`, `timbre`, `morph`: the macro controls, 0 to 1
/// - `level`: output level, 0 to 1
/// - `decay`: how long the gate takes to fall 60 dB after a trigger, in seconds
pub struct MacroOscillator {
    sample_rate: f32,
    cells: Arc<ControlCells>,

    model: usize,
    frequency: f32,
    frequency_mod_depth: f32,
    harmonics: f32,
    timbre: f32,
    morph: f32,
    level: LinearRamp,
    /// `decay` on the gate's tuned 0-to-1 scale.
    decay: f32,

    voice: Voice,
    /// The last frequency turned into a pitch, and that pitch.
    last_hz: f32,
    last_note: f32,

    inputs: [[f32; MAX_BLOCK]; 4],
    connected: [bool; 4],
    audio: [f32; MAX_BLOCK],
    aux: [f32; MAX_BLOCK],
}

impl MacroOscillator {
    /// A macro oscillator holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut module = Self {
            sample_rate: sample_rate.max(1) as f32,
            cells,
            model: 0,
            frequency: 440.0,
            frequency_mod_depth: 0.0,
            harmonics: 0.5,
            timbre: 0.5,
            morph: 0.5,
            level: LinearRamp::new(1.0, gain_ramp_frames(sample_rate)),
            decay: 0.5,
            voice: Voice::new(sample_rate, 0),
            last_hz: 440.0,
            last_note: 69.0,
            inputs: [[0.0; MAX_BLOCK]; 4],
            connected: [false; 4],
            audio: [0.0; MAX_BLOCK],
            aux: [0.0; MAX_BLOCK],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = module.cells.load(index) {
                let _ = apply_declared(&mut module, index, value);
            }
        }
        // The configured model and level are where the voice starts, not
        // a fade.
        module.voice = Voice::new(sample_rate, module.model);
        module.level.settle();
        module
    }

    /// The pitch in semitones for `hz`, reusing the last one when the
    /// frequency hasn't moved.
    fn note_for(&mut self, hz: f32) -> f32 {
        let hz = hz.clamp(1.0, 0.5 * self.sample_rate);
        if hz.to_bits() != self.last_hz.to_bits() {
            self.last_hz = hz;
            self.last_note = 69.0 + 12.0 * (hz / 440.0).log2();
        }
        self.last_note
    }

    /// The frequency for frames `range`: their mean, so audio-rate FM is
    /// smoothed rather than sampled once per engine block.
    fn block_frequency(&self, range: std::ops::Range<usize>) -> f32 {
        let n = range.len() as f32;
        let fm = &self.inputs[FREQUENCY_MOD_IN][range.clone()];
        let fm = fm.iter().sum::<f32>() / n * self.frequency_mod_depth;
        let base = if self.connected[FREQUENCY_IN] {
            self.inputs[FREQUENCY_IN][range].iter().sum::<f32>() / n
        } else {
            self.frequency
        };
        base + fm
    }

    /// What drives the gate over frames `range`.
    fn block_gate(&mut self, range: std::ops::Range<usize>) -> Gate {
        if self.connected[LEVEL_IN] {
            let n = range.len() as f32;
            let level = self.inputs[LEVEL_IN][range].iter().sum::<f32>() / n;
            Gate::Level(level)
        } else if self.connected[TRIGGER_IN] {
            let mut rising = false;
            for i in range {
                rising |= self.voice.trigger_edge(self.inputs[TRIGGER_IN][i]);
            }
            Gate::Ping { rising }
        } else {
            Gate::Open
        }
    }
}

impl Module for MacroOscillator {
    fn name(&self) -> &str {
        "MacroOscillator"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        let mut start = 0;
        while start < frames {
            let end = (start + ENGINE_BLOCK).min(frames);
            let hz = self.block_frequency(start..end);
            let block = Block {
                model: self.model,
                note: self.note_for(hz),
                harmonics: self.harmonics,
                timbre: self.timbre,
                morph: self.morph,
                gate: self.block_gate(start..end),
                decay: self.decay,
            };
            self.voice.render(
                &block,
                &mut self.audio[start..end],
                &mut self.aux[start..end],
            );
            for i in start..end {
                let level = self.level.next_value();
                self.audio[i] = (self.audio[i] * level).clamp(-1.0, 1.0);
                self.aux[i] = (self.aux[i] * level).clamp(-1.0, 1.0);
            }
            start = end;
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
        &mut self.inputs[index.min(INPUTS.len() - 1)]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.audio,
            _ => &self.aux,
        }
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.inputs[index].fill(value);
        self.connected[index] = true;
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio[0]),
            "aux" => Ok(self.aux[0]),
            _ => Err(format!("Unknown output port: {port}")),
        }
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        if let Some(flag) = self.connected.get_mut(index) {
            *flag = connected;
        }
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let macro_value = |v: f32| v.clamp(0.0, 1.0);
        let applied = match (control, value) {
            (MODEL, RtValue::U32(position)) => {
                let position = position as usize;
                if position >= MODEL_COUNT {
                    return Err(Refusal::Invalid);
                }
                self.model = position;
                value
            }
            (FREQUENCY, RtValue::F32(hz)) => {
                self.frequency = hz.max(0.0);
                RtValue::F32(self.frequency)
            }
            (FREQUENCY_MOD_DEPTH, RtValue::F32(depth)) => {
                self.frequency_mod_depth = depth;
                value
            }
            (HARMONICS, RtValue::F32(v)) => {
                self.harmonics = macro_value(v);
                RtValue::F32(self.harmonics)
            }
            (TIMBRE, RtValue::F32(v)) => {
                self.timbre = macro_value(v);
                RtValue::F32(self.timbre)
            }
            (MORPH, RtValue::F32(v)) => {
                self.morph = macro_value(v);
                RtValue::F32(self.morph)
            }
            (LEVEL, RtValue::F32(v)) => {
                let level = v.clamp(0.0, 1.0);
                self.level.set_target(level);
                RtValue::F32(level)
            }
            (DECAY, RtValue::F32(seconds)) => {
                let seconds = seconds.clamp(MIN_DECAY, MAX_DECAY);
                self.decay = (seconds / DECAY_AT_ZERO_SECONDS).log2() / DECAY_DOUBLINGS_PER_UNIT;
                RtValue::F32(seconds)
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
