//! Resonator: tuned, ringing bodies excited by any sound.
//!
//! A click, a drum, a voice or noise goes in; a bowl, a bell, a plucked
//! string or a string with a halo of sympathetic strings rings out, in
//! tune. Strummed by a sequencer with nothing patched into `audio`, its
//! internal exciter makes it a melodic voice of its own; fed other audio, it
//! tunes that audio to the played pitch.

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
    BRIGHTNESS, DAMPING, FREQUENCY, LEVEL, MODEL, POLYPHONY, POSITION, STRUCTURE, TABLE,
};
use self::limiter::Limiter;
use self::voices::{Patch, Performance, Voices, MODELS, VOICES};

pub use self::tuning::ring_seconds;

mod controls;
mod limiter;
mod plucker;
mod sympathetic;
#[cfg(test)]
mod tests;
mod tuning;
mod voices;

const TYPE_ID: &str = "resonator";

/// The voices render in blocks of this many frames, whatever the host's
/// block size: 0.5 ms at 48 kHz.
pub(super) const ENGINE_BLOCK: usize = 24;

/// Input port names, in `input_block_mut` order.
const INPUTS: [&str; 3] = ["audio", "frequency", "strum"];
const AUDIO_IN: usize = 0;
const FREQUENCY_IN: usize = 1;
const STRUM_IN: usize = 2;

/// Output port names, in `output_block` order.
const OUTPUTS: [&str; 3] = ["odd", "even", "audio"];

/// A strum is a rise above `STRUM_HIGH`, re-armed below `STRUM_LOW`.
const STRUM_HIGH: f32 = 0.3;
const STRUM_LOW: f32 = 0.1;

/// How long a `structure`, `brightness`, `damping` or `position` write
/// takes to arrive. Landing at once would step the sound at the next
/// engine block, heard as zipper noise in a sweep.
const SHAPE_GLIDE_SECONDS: f32 = 0.01;

/// Factory for constructing resonators from configuration.
pub struct ResonatorFactory;

impl ModuleFactory for ResonatorFactory {
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
        let module = Resonator::with_cells(sample_rate, cells);
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// Tuned bodies that ring when struck.
///
/// # Inputs
/// - `audio`: the sound that excites the resonator; while it is connected
///   the internal exciter is off
/// - `frequency`: pitch in Hz; when connected it overrides the control
/// - `strum`: each rising edge strikes (with the internal exciter) and, at
///   polyphony above 1, moves to the next voice
///
/// The voices render in blocks of 24 frames, so the output runs 24 frames
/// (0.5 ms at 48 kHz) behind the inputs and sounds the same at any block
/// size.
///
/// # Outputs
/// - `odd`, `even`: at polyphony 1, the odd and even partials (or the
///   string and its pickup); above 1, alternate voices, for stereo
/// - `audio`: the two summed
///
/// # Controls
/// - `model`: modal, sympathetic_strings or string; the old body rings out
/// - `frequency`: pitch in Hz
/// - `structure`, `brightness`, `damping`, `position`: the body, 0 to 1;
///   a write glides over 10 ms
/// - `polyphony`: voices ringing at once, 1 to 4
/// - `level`: output level, 0 to 1
pub struct Resonator {
    sample_rate: f32,
    cells: Arc<ControlCells>,

    model: usize,
    frequency: f32,
    polyphony: usize,
    structure: LinearRamp,
    brightness: LinearRamp,
    damping: LinearRamp,
    position: LinearRamp,
    /// Whether a block has been rendered: writes before then land at once.
    heard: bool,
    level: LinearRamp,

    voices: Voices,
    limiter: Limiter,

    /// Inputs gathered for the next engine block.
    gathered: usize,
    frequency_sum: f32,
    /// Frames in `frequency_sum`: from the block's start, or from a strum
    /// within it, so a new voice starts at the pitch that came with it.
    frequency_frames: usize,
    strum_high: bool,
    rising: bool,
    excitation: [f32; ENGINE_BLOCK],
    /// The last engine block, played out while the next one gathers.
    ready_odd: [f32; ENGINE_BLOCK],
    ready_even: [f32; ENGINE_BLOCK],

    inputs: [[f32; MAX_BLOCK]; 3],
    connected: [bool; 3],
    odd: [f32; MAX_BLOCK],
    even: [f32; MAX_BLOCK],
    audio: [f32; MAX_BLOCK],
}

impl Resonator {
    /// A resonator holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let glide = ((sample_rate as f32 * SHAPE_GLIDE_SECONDS / ENGINE_BLOCK as f32).round()
            as u32)
            .max(1);
        let polyphony = match cells.load(POLYPHONY) {
            Some(RtValue::I32(voices)) => voices.clamp(1, VOICES as i32) as usize,
            _ => 1,
        };
        let mut module = Self {
            sample_rate: sample_rate.max(1) as f32,
            cells,
            model: 0,
            frequency: 220.0,
            polyphony,
            structure: LinearRamp::new(0.25, glide),
            brightness: LinearRamp::new(0.5, glide),
            damping: LinearRamp::new(0.5, glide),
            position: LinearRamp::new(0.3, glide),
            heard: false,
            level: LinearRamp::new(1.0, gain_ramp_frames(sample_rate)),
            voices: Voices::new(sample_rate, polyphony),
            limiter: Limiter::new(sample_rate),
            gathered: 0,
            frequency_sum: 0.0,
            frequency_frames: 0,
            strum_high: false,
            rising: false,
            excitation: [0.0; ENGINE_BLOCK],
            ready_odd: [0.0; ENGINE_BLOCK],
            ready_even: [0.0; ENGINE_BLOCK],
            inputs: [[0.0; MAX_BLOCK]; 3],
            connected: [false; 3],
            odd: [0.0; MAX_BLOCK],
            even: [0.0; MAX_BLOCK],
            audio: [0.0; MAX_BLOCK],
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = module.cells.load(index) {
                let _ = apply_declared(&mut module, index, value);
            }
        }
        module.level.settle();
        module
    }

    /// Gathers frame `i` of the inputs into the next engine block.
    fn gather(&mut self, i: usize) {
        let hz = if self.connected[FREQUENCY_IN] {
            self.inputs[FREQUENCY_IN][i]
        } else {
            self.frequency
        };
        let strum = self.inputs[STRUM_IN][i];
        if !self.strum_high && strum > STRUM_HIGH {
            self.strum_high = true;
            self.rising = true;
            self.frequency_sum = 0.0;
            self.frequency_frames = 0;
        } else if self.strum_high && strum < STRUM_LOW {
            self.strum_high = false;
        }
        // Bounded, so a wild sample can't overflow the block's sum; one
        // that is not a number counts as the control.
        self.frequency_frames += 1;
        self.frequency_sum += if hz.is_nan() {
            self.frequency
        } else {
            hz.clamp(0.0, self.sample_rate)
        };
        let excitation = self.inputs[AUDIO_IN][i];
        self.excitation[self.gathered] = if excitation.is_finite() {
            excitation.clamp(-16.0, 16.0)
        } else {
            0.0
        };
        self.gathered += 1;
    }

    /// Renders the gathered inputs into the next engine block.
    fn render_block(&mut self) {
        let hz = self.frequency_sum / self.frequency_frames.max(1) as f32;
        let performance = Performance {
            model: self.model,
            polyphony: self.polyphony,
            frequency: hz / self.sample_rate,
            strum: self.rising,
            internal: !self.connected[AUDIO_IN],
        };
        let patch = self.next_patch();
        self.voices.render(
            &performance,
            &patch,
            &self.excitation,
            &mut self.ready_odd,
            &mut self.ready_even,
        );
        self.heard = true;
        self.gathered = 0;
        self.frequency_sum = 0.0;
        self.frequency_frames = 0;
        self.rising = false;
    }

    /// The shape controls for the next engine block, one glide step on.
    fn next_patch(&mut self) -> Patch {
        Patch {
            structure: self.structure.next_value(),
            brightness: self.brightness.next_value(),
            damping: self.damping.next_value(),
            position: self.position.next_value(),
        }
    }

    /// Moves a shape control: at once before anything is heard, gliding after.
    fn shape(&mut self, control: ControlIndex, value: f32) -> f32 {
        let value = value.clamp(0.0, 1.0);
        let heard = self.heard;
        let ramp = match control {
            STRUCTURE => &mut self.structure,
            BRIGHTNESS => &mut self.brightness,
            DAMPING => &mut self.damping,
            _ => &mut self.position,
        };
        if heard {
            ramp.set_target(value);
        } else {
            ramp.snap(value);
        }
        value
    }
}

impl Module for Resonator {
    fn name(&self) -> &str {
        "Resonator"
    }

    fn process(&mut self, frames: usize) -> bool {
        let frames = frames.min(MAX_BLOCK);
        for i in 0..frames {
            // Play the last block while gathering the next one.
            let k = self.gathered;
            let (odd, even) = self.limiter.process(self.ready_odd[k], self.ready_even[k]);
            let level = self.level.next_value();
            let (odd, even) = (odd * level, even * level);
            self.odd[i] = odd.clamp(-1.0, 1.0);
            self.even[i] = even.clamp(-1.0, 1.0);
            self.audio[i] = (odd + even).clamp(-1.0, 1.0);
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
            0 => &self.odd,
            1 => &self.even,
            _ => &self.audio,
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
            "odd" => Ok(self.odd[0]),
            "even" => Ok(self.even[0]),
            "audio" => Ok(self.audio[0]),
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
        let applied = match (control, value) {
            (MODEL, RtValue::U32(position)) => {
                if position as usize >= MODELS.len() {
                    return Err(Refusal::Invalid);
                }
                self.model = position as usize;
                value
            }
            (FREQUENCY, RtValue::F32(hz)) => {
                self.frequency = hz.max(0.0);
                RtValue::F32(self.frequency)
            }
            (STRUCTURE | BRIGHTNESS | DAMPING | POSITION, RtValue::F32(v)) => {
                RtValue::F32(self.shape(control, v))
            }
            (POLYPHONY, RtValue::I32(voices)) => {
                let voices = voices.clamp(1, VOICES as i32);
                self.polyphony = voices as usize;
                RtValue::I32(voices)
            }
            (LEVEL, RtValue::F32(v)) => {
                let level = v.clamp(0.0, 1.0);
                self.level.set_target(level);
                RtValue::F32(level)
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
