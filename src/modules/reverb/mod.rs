//! Stereo reverb: a multichannel diffuser feeding a feedback delay network.
//!
//! The input is spread over eight channels and diffused into a dense cloud
//! of early reflections, which also feeds an eight-line feedback delay
//! network for the tail (see [`network`] for the design and its sources).
//!
//! # Features
//!
//! - Stereo in/out with configurable width
//! - Separate early reflections and reverb tail
//! - Room size, decay time, damping controls; room size and decay glide to
//!   new values rather than jumping, so both can be automated
//! - Freeze mode for infinite sustain
//! - All delay buffers pre-allocated at construction (allocation-free processing)
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth" } },
//!     { "id": "reverb", "type": "reverb", "config": { "room_size": 0.5, "decay": 0.6, "wet": 0.4, "dry": 0.8 } },
//!     { "id": "dac", "type": "dac" }
//!   ],
//!   "connections": [
//!     { "from": "osc", "from_port": "audio", "to": "reverb", "to_port": "audio_left" },
//!     { "from": "reverb", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
//!     { "from": "reverb", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
//!   ]
//! }
//! ```

use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::dsp::{Damper, LinearRamp};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::mixer::gain_ramp_frames;
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{DAMPING, DECAY, DRY, FREEZE, ROOM_SIZE, TABLE, WET, WIDTH};
use self::network::{tap, Diffuser, FeedbackNetwork, Frame};
use self::tuning::{Retuning, Tuning};

mod controls;
mod inputs;
mod network;
mod outputs;
mod tuning;

/// How strongly `damping` filters the input before it enters the reverb,
/// as a fraction of the one-pole coefficient.
const INPUT_DAMPING: f32 = 0.4;
/// How strongly `damping` filters each trip round the feedback network.
/// Applied once per trip, so long lines darken less per second than short
/// ones, as in a real room where high frequencies are lost at each wall.
const LOOP_DAMPING: f32 = 0.6;
/// Each side's input is spread over half the channels; this keeps the
/// energy entering the network equal to the energy of the input.
const INPUT_SPREAD: f32 = 0.5;
/// Level of the early reflections against the tail.
const EARLY_LEVEL: f32 = 0.5;
/// Overall wet level, chosen so a sustained sound comes back at about the
/// level it went in with the default controls.
const WET_LEVEL: f32 = 1.7;

/// The output taps, as weights over the eight channels. Each is orthogonal
/// to the others of its pair, so with full width the two sides of the tail
/// (and of the early reflections) are uncorrelated.
const S: f32 = 0.353_553_4; // 1/sqrt(8)
const TAIL_LEFT: Frame = [S, -S, S, -S, S, -S, S, -S];
const TAIL_RIGHT: Frame = [S, S, -S, -S, S, S, -S, -S];
const EARLY_LEFT: Frame = [S, -S, S, S, S, -S, -S, S];
const EARLY_RIGHT: Frame = [S, S, S, -S, -S, -S, S, S];

// --- Main reverb module ---

/// A stereo reverb: an eight-channel diffuser for the early reflections
/// feeding an eight-line feedback delay network for the tail.
///
/// # Inputs
///
/// - `audio_left` - Left audio input
/// - `audio_right` - Right audio input (copies the left if not connected; mono input is fine)
///
/// # Outputs
///
/// - `audio_left` - Left audio output
/// - `audio_right` - Right audio output
///
/// # Controls
///
/// - `room_size` - Room size (0.0–1.0, maps to 1–100m, default 0.5). A change
///   crossfades to the new room over 30 ms, so it can be automated.
/// - `decay` - Reverb decay time (0.0–1.0, maps to RT60 0.2–15s, default 0.5),
///   crossfaded like `room_size`
/// - `damping` - High-frequency damping (0.0–1.0, default 0.5)
/// - `wet` - Wet signal level (0.0–1.0, default 0.33)
/// - `dry` - Dry signal level (0.0–1.0, default 1.0)
/// - `width` - Stereo width (0.0–1.0, default 1.0)
/// - `freeze` - Infinite hold mode (bool, default false)
pub struct Reverb {
    sample_rate: u32,

    // Controls, applied on the thread running the reverb.
    room_size: f32,
    decay: f32,
    damping: f32,
    /// The output mix, gliding to each write as a mixer's fader does: a
    /// gain that stepped would click.
    wet: LinearRamp,
    dry: LinearRamp,
    width: LinearRamp,
    freeze: bool,
    /// 1 while the reverb listens, 0 while frozen: it fades the input, the
    /// early reflections and the loop's damping out as freeze engages, over
    /// the same time the tuning crossfades, so freezing never clicks.
    listening: LinearRamp,
    /// What `room_size`, `decay` and `freeze` make of the delay network,
    /// recomputed as they change rather than every sample, and crossfaded
    /// from the last tuning so a change never steps the output.
    retuning: Retuning,
    cells: Arc<ControlCells>,

    inputs: inputs::ReverbInputs,
    outputs: outputs::ReverbOutputs,

    // Signal path.
    input_filters: [Damper; 2],
    diffuser: Diffuser,
    network: FeedbackNetwork,
}

impl Reverb {
    /// Creates a new Reverb with default controls.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// A reverb holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        // Every delay holds the largest offset any control can ask of it,
        // plus one for the read that comes before the write.
        let largest = Tuning::largest(sample_rate);
        let diffusion = largest
            .diffusion
            .map(|stage| stage.map(|offset| offset + 1));
        let lines = largest.lines.map(|len| len + 1);
        let fade = Retuning::fade_frames(sample_rate);
        let ramp = gain_ramp_frames(sample_rate);
        let mut reverb = Self {
            sample_rate,
            room_size: 0.5,
            decay: 0.5,
            damping: 0.5,
            wet: LinearRamp::new(0.33, ramp),
            dry: LinearRamp::new(1.0, ramp),
            width: LinearRamp::new(1.0, ramp),
            freeze: false,
            listening: LinearRamp::new(1.0, fade),
            retuning: Retuning::settled(Tuning::new(sample_rate, 0.5, 0.5, false), sample_rate),
            cells,
            inputs: inputs::ReverbInputs::new(),
            outputs: outputs::ReverbOutputs::new(),
            input_filters: [Damper::new(), Damper::new()],
            diffuser: Diffuser::new(&diffusion),
            network: FeedbackNetwork::new(&lines),
        };
        for control in [ROOM_SIZE, DECAY, DAMPING, WET, DRY, WIDTH, FREEZE] {
            if let Some(value) = reverb.cells.load(control) {
                let _ = apply_declared(&mut reverb, control, value);
            }
        }
        // Built, the reverb starts where its controls are, not gliding there.
        reverb.retuning = Retuning::settled(reverb.tuning(), sample_rate);
        reverb.listening.settle();
        reverb.wet.settle();
        reverb.dry.settle();
        reverb.width.settle();
        reverb
    }

    /// The tuning the current controls ask for.
    fn tuning(&self) -> Tuning {
        Tuning::new(self.sample_rate, self.room_size, self.decay, self.freeze)
    }

    #[inline]
    fn process_sample(&mut self, i: usize) {
        let left = self.inputs.left(i);
        let right = if self.inputs.right_active() {
            self.inputs.right(i)
        } else {
            left
        };

        let s = self.retuning.next_weight();
        let (from, to) = (self.retuning.from(), self.retuning.to());
        let listening = self.listening.next_value();

        // Spread each side over alternate channels, filtered and faded out
        // while frozen.
        let band = self.damping * INPUT_DAMPING;
        let feed = listening * INPUT_SPREAD;
        let feed_left = self.input_filters[0].tick(left, band) * feed;
        let feed_right = self.input_filters[1].tick(right, band) * feed;
        let mut frame: Frame =
            std::array::from_fn(|ch| if ch % 2 == 0 { feed_left } else { feed_right });

        self.diffuser.process(&mut frame, from, to, s);
        let loop_damping = self.damping * LOOP_DAMPING * listening;
        let tail = self.network.process(&frame, from, to, s, loop_damping);

        let early = EARLY_LEVEL * listening;
        let wet_left = WET_LEVEL * (tap(&tail, &TAIL_LEFT) + early * tap(&frame, &EARLY_LEFT));
        let wet_right = WET_LEVEL * (tap(&tail, &TAIL_RIGHT) + early * tap(&frame, &EARLY_RIGHT));

        let wet = self.wet.next_value();
        let dry = self.dry.next_value();
        let width = self.width.next_value();
        let same = wet * (0.5 + 0.5 * width);
        let cross = wet * 0.5 * (1.0 - width);
        let out_left = dry * left + same * wet_left + cross * wet_right;
        let out_right = dry * right + same * wet_right + cross * wet_left;
        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
        self.outputs.set(i, finite(out_left), finite(out_right));
    }
}

impl Module for Reverb {
    fn name(&self) -> &str {
        "Reverb"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            self.process_sample(i);
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
            (FREEZE, RtValue::Bool(freeze)) => {
                self.freeze = freeze;
                self.listening.set_target(if freeze { 0.0 } else { 1.0 });
                value
            }
            (WET | DRY | WIDTH, RtValue::F32(value)) => {
                let value = value.clamp(0.0, 1.0);
                match control {
                    WET => self.wet.set_target(value),
                    DRY => self.dry.set_target(value),
                    _ => self.width.set_target(value),
                }
                RtValue::F32(value)
            }
            (_, RtValue::F32(value)) => {
                let held = match control {
                    ROOM_SIZE => &mut self.room_size,
                    DECAY => &mut self.decay,
                    DAMPING => &mut self.damping,
                    _ => return Err(Refusal::Unsupported),
                };
                *held = value.clamp(0.0, 1.0);
                RtValue::F32(*held)
            }
            _ => return Err(Refusal::Unsupported),
        };
        if matches!(control, ROOM_SIZE | DECAY | FREEZE) {
            self.retuning.retarget(self.tuning());
        }
        Ok(applied)
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        // As a number, freeze has always meant on above one half.
        let value = match key {
            "freeze" => f32::from(u8::from(value > 0.5)),
            _ => value,
        };
        local_set(self, key, value)
    }
}

/// Factory for constructing Reverb modules from configuration.
pub struct ReverbFactory;

const TYPE_ID: &str = "reverb";
/// The number controls' config keys, in control index order, then `freeze`.
const CONFIG_KEYS: &[ConfigKey] = &[
    ConfigKey::float("room_size"),
    ConfigKey::float("decay"),
    ConfigKey::float("damping"),
    ConfigKey::float("wet"),
    ConfigKey::float("dry"),
    ConfigKey::float("width"),
    ConfigKey::boolean("freeze"),
];

impl ModuleFactory for ReverbFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        CONFIG_KEYS
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let cells = Arc::new(ControlCells::new(controls::defaults()));
        for (index, key) in CONFIG_KEYS[..FREEZE.0 as usize].iter().enumerate() {
            if let Some(value) = reader.float(key)? {
                cells.publish(ControlIndex(index as u16), RtValue::F32(value));
            }
        }
        if let Some(freeze) = config.get("freeze").and_then(|v| v.as_bool()) {
            cells.publish(FREEZE, RtValue::Bool(freeze));
        }
        let reverb = Reverb::with_cells(sample_rate, cells.clone());
        let surface = DeclaredSurface::new(TABLE.clone(), cells);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(reverb)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

#[cfg(test)]
mod tests;
