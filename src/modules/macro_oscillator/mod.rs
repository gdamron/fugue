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
    DECAY, FREQUENCY, FREQUENCY_MOD_DEPTH, HARMONICS, KEY, LEVEL, MAX_DECAY, MIN_DECAY, MODEL,
    MORPH, SCALE, TABLE, TIMBRE,
};
use self::engine::ENGINE_BLOCK;
use self::engines::MODEL_COUNT;
use self::voice::{Block, Gate, Voice};
use crate::music::scales::{KEY_NAMES, SCALE_NAMES};

mod controls;
mod engine;
mod engines;
#[cfg(test)]
mod glide_tests;
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

/// How long a `harmonics`, `timbre` or `morph` write takes to arrive, in
/// seconds. A write that landed at once would step the sound at the next
/// engine block; in `virtual_analog` a `harmonics` sweep would be a
/// staircase of detunes, heard as zipper.
const MACRO_GLIDE_SECONDS: f32 = 0.01;

/// How many engine blocks a macro write glides over at `sample_rate`.
fn macro_glide_blocks(sample_rate: u32) -> u32 {
    let frames = sample_rate as f32 * MACRO_GLIDE_SECONDS;
    ((frames / ENGINE_BLOCK as f32).round() as u32).max(1)
}

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
/// - `trigger`: each rising edge pings the low-pass gate (ignored while
///   `level` is connected)
/// - `level`: when connected, how far the low-pass gate is open, 0 to 1
///
/// With neither `trigger` nor `level` connected the voice sounds
/// continuously, with no gate; patching or unpatching them fades.
///
/// The models render in blocks of 12 frames, whatever the host's block
/// size, so the output runs 12 frames (0.25 ms at 48 kHz) behind the
/// inputs and sounds the same at any block size.
///
/// # Outputs
/// - `audio`: the model's main sound
/// - `aux`: the model's variant sound
///
/// # Controls
/// - `model`: the synthesis model; a change crossfades over 20 ms
/// - `frequency`, `frequency_mod_depth`: pitch and FM depth in Hz
/// - `harmonics`, `timbre`, `morph`: the macro controls, 0 to 1; a write
///   glides over 10 ms
/// - `level`: output level, 0 to 1
/// - `decay`: how long the gate takes to fall 60 dB after a trigger, in seconds
/// - `key`, `scale`: the key and scale the `chords` model builds chords in
pub struct MacroOscillator {
    sample_rate: f32,
    cells: Arc<ControlCells>,

    model: usize,
    frequency: f32,
    frequency_mod_depth: f32,
    /// The macro controls, gliding once per engine block. Each engine
    /// spreads its block's values across the block's frames.
    harmonics: LinearRamp,
    /// Where `harmonics` is gliding to.
    harmonics_target: f32,
    timbre: LinearRamp,
    morph: LinearRamp,
    /// Whether a block has been rendered: writes before then (the config,
    /// a development's starting values) land at once.
    heard: bool,
    level: LinearRamp,
    /// `decay` on the gate's tuned 0-to-1 scale.
    decay: f32,
    key: u8,
    scale: u8,

    voice: Voice,
    /// The last frequency turned into a pitch, and that pitch.
    last_hz: f32,
    last_note: f32,

    /// Inputs gathered for the next engine block: how many frames, the
    /// sums of frequency (with FM) and level, and whether a trigger rose.
    gathered: usize,
    frequency_sum: f32,
    level_sum: f32,
    rising: bool,
    /// The last engine block, played out while the next one gathers.
    ready_audio: [f32; ENGINE_BLOCK],
    ready_aux: [f32; ENGINE_BLOCK],

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
            harmonics: LinearRamp::new(0.5, macro_glide_blocks(sample_rate)),
            harmonics_target: 0.5,
            timbre: LinearRamp::new(0.5, macro_glide_blocks(sample_rate)),
            morph: LinearRamp::new(0.5, macro_glide_blocks(sample_rate)),
            heard: false,
            level: LinearRamp::new(1.0, gain_ramp_frames(sample_rate)),
            decay: 0.5,
            key: 0,
            scale: 0,
            voice: Voice::new(sample_rate, 0),
            last_hz: 440.0,
            last_note: 69.0,
            gathered: 0,
            frequency_sum: 0.0,
            level_sum: 0.0,
            rising: false,
            ready_audio: [0.0; ENGINE_BLOCK],
            ready_aux: [0.0; ENGINE_BLOCK],
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
        module.voice.start_on(module.model);
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

    /// Gathers frame `i` of the inputs into the next engine block.
    fn gather(&mut self, i: usize) {
        let mut hz = if self.connected[FREQUENCY_IN] {
            self.inputs[FREQUENCY_IN][i]
        } else {
            self.frequency
        };
        if self.connected[FREQUENCY_MOD_IN] {
            hz += self.inputs[FREQUENCY_MOD_IN][i] * self.frequency_mod_depth;
        }
        // Bounded, so a wild sample can't overflow the block's sum; one
        // that is not a number holds the last pitch.
        self.frequency_sum += if hz.is_nan() {
            self.last_hz
        } else {
            hz.clamp(0.0, self.sample_rate)
        };
        let level = self.inputs[LEVEL_IN][i];
        self.level_sum += if level.is_nan() {
            0.0
        } else {
            level.clamp(-16.0, 16.0)
        };
        // Edges are tracked even while `level` rules, so unpatching it
        // doesn't fire a stale trigger.
        self.rising |= self.voice.trigger_edge(self.inputs[TRIGGER_IN][i]);
        self.gathered += 1;
    }

    /// Renders the gathered inputs into the next engine block.
    fn render_block(&mut self) {
        let n = self.gathered.max(1) as f32;
        let gate = if self.connected[LEVEL_IN] {
            Gate::Level(self.level_sum / n)
        } else if self.connected[TRIGGER_IN] {
            Gate::Ping {
                rising: self.rising,
            }
        } else {
            Gate::Open
        };
        let block = Block {
            model: self.model,
            note: self.note_for(self.frequency_sum / n),
            harmonics: self.harmonics.next_value(),
            harmonics_target: self.harmonics_target,
            timbre: self.timbre.next_value(),
            morph: self.morph.next_value(),
            gate,
            decay: self.decay,
            key: self.key,
            scale: self.scale,
        };
        self.voice
            .render(&block, &mut self.ready_audio, &mut self.ready_aux);
        self.heard = true;
        self.gathered = 0;
        self.frequency_sum = 0.0;
        self.level_sum = 0.0;
        self.rising = false;
    }
}

impl Module for MacroOscillator {
    fn name(&self) -> &str {
        "MacroOscillator"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        for i in 0..frames {
            // Play the last block while gathering the next one.
            let level = self.level.next_value();
            let k = self.gathered;
            self.audio[i] = (self.ready_audio[k] * level).clamp(-1.0, 1.0);
            self.aux[i] = (self.ready_aux[k] * level).clamp(-1.0, 1.0);
            self.gather(i);
            if self.gathered == ENGINE_BLOCK {
                self.render_block();
            }
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
            (HARMONICS | TIMBRE | MORPH, RtValue::F32(v)) => {
                let v = macro_value(v);
                if control == HARMONICS {
                    self.harmonics_target = v;
                }
                let heard = self.heard;
                let ramp = match control {
                    HARMONICS => &mut self.harmonics,
                    TIMBRE => &mut self.timbre,
                    _ => &mut self.morph,
                };
                if heard {
                    ramp.set_target(v);
                } else {
                    ramp.snap(v);
                }
                RtValue::F32(v)
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
            (KEY, RtValue::U32(position)) if (position as usize) < KEY_NAMES.len() => {
                self.key = position as u8;
                value
            }
            (SCALE, RtValue::U32(position)) if (position as usize) < SCALE_NAMES.len() => {
                self.scale = position as u8;
                value
            }
            (KEY | SCALE, RtValue::U32(_)) => return Err(Refusal::Invalid),
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
