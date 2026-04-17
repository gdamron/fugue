//! Freeverb reverb module.
//!
//! An implementation of the Freeverb algorithm (Jezar at Dreampoint) providing
//! a classic algorithmic reverb. The architecture uses 8 parallel comb filters
//! feeding into 4 cascaded allpass filters per stereo channel.
//!
//! # Features
//!
//! - Stereo in/out with configurable width
//! - Room size, damping, wet/dry mix controls
//! - Freeze mode for infinite sustain
//! - All delay buffers pre-allocated at construction (allocation-free processing)
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc", "type": "oscillator", "config": { "oscillator_type": "sawtooth" } },
//!     { "id": "reverb", "type": "reverb", "config": { "room_size": 0.7, "damping": 0.4, "wet": 0.5, "dry": 0.8 } },
//!     { "id": "dac", "type": "dac" }
//!   ],
//!   "connections": [
//!     { "from": "osc", "from_port": "audio", "to": "reverb", "to_port": "left" },
//!     { "from": "reverb", "from_port": "left", "to": "dac", "to_port": "left" },
//!     { "from": "reverb", "from_port": "right", "to": "dac", "to_port": "right" }
//!   ]
//! }
//! ```

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::ReverbControls;

mod controls;
mod inputs;
mod outputs;

// --- Freeverb tuning constants ---

const FIXED_GAIN: f32 = 0.015;
const SCALE_WET: f32 = 3.0;
const SCALE_DAMPING: f32 = 0.4;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const STEREO_SPREAD: usize = 23;
const ALLPASS_FEEDBACK: f32 = 0.5;

const COMB_SIZES: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_SIZES: [usize; 4] = [556, 441, 341, 225];

// --- DSP primitives ---

/// Circular delay buffer. Pre-allocated at construction.
struct DelayLine {
    buffer: Vec<f32>,
    index: usize,
}

impl DelayLine {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length],
            index: 0,
        }
    }

    #[inline]
    fn read(&self) -> f32 {
        self.buffer[self.index]
    }

    #[inline]
    fn write_and_advance(&mut self, value: f32) {
        self.buffer[self.index] = value;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
    }
}

/// Lowpass-feedback comb filter.
struct CombFilter {
    delay: DelayLine,
    filter_state: f32,
}

impl CombFilter {
    fn new(length: usize) -> Self {
        Self {
            delay: DelayLine::new(length),
            filter_state: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let output = self.delay.read();
        self.filter_state = output * (1.0 - damping) + self.filter_state * damping;
        self.delay.write_and_advance(input + self.filter_state * feedback);
        output
    }
}

/// Schroeder allpass filter with fixed 0.5 feedback.
struct AllpassFilter {
    delay: DelayLine,
}

impl AllpassFilter {
    fn new(length: usize) -> Self {
        Self {
            delay: DelayLine::new(length),
        }
    }

    #[inline]
    fn tick(&mut self, input: f32) -> f32 {
        let delayed = self.delay.read();
        let output = -input + delayed;
        self.delay
            .write_and_advance(input + delayed * ALLPASS_FEEDBACK);
        output
    }
}

// --- Main reverb module ---

/// Scales a base delay length for the given sample rate.
fn scale_length(base: usize, sample_rate: u32) -> usize {
    ((base as u64 * sample_rate as u64) / 44100).max(1) as usize
}

/// A stereo reverb effect using the Freeverb algorithm.
///
/// # Inputs
///
/// - `left` - Left audio input
/// - `right` - Right audio input (0.0 if not connected; mono input is fine)
///
/// # Outputs
///
/// - `left` - Left audio output
/// - `right` - Right audio output
///
/// # Controls
///
/// - `room_size` - Room size / decay length (0.0–1.0, default 0.5)
/// - `damping` - High-frequency damping (0.0–1.0, default 0.5)
/// - `wet` - Wet signal level (0.0–1.0, default 0.33)
/// - `dry` - Dry signal level (0.0–1.0, default 1.0)
/// - `width` - Stereo width (0.0–1.0, default 1.0)
/// - `freeze` - Infinite hold mode (bool, default false)
pub struct Reverb {
    ctrl: ReverbControls,

    // 8 comb filters per channel
    combs_l: [CombFilter; 8],
    combs_r: [CombFilter; 8],

    // 4 allpass filters per channel
    allpasses_l: [AllpassFilter; 4],
    allpasses_r: [AllpassFilter; 4],

    inputs: inputs::ReverbInputs,
    outputs: outputs::ReverbOutputs,
    last_processed_sample: u64,
}

impl Reverb {
    /// Creates a new Reverb with default controls.
    pub fn new(sample_rate: u32) -> Self {
        let controls = ReverbControls::new(0.5, 0.5, 0.33, 1.0, 1.0, false);
        Self::new_with_controls(sample_rate, controls)
    }

    /// Creates a new Reverb with the given controls.
    pub fn new_with_controls(sample_rate: u32, controls: ReverbControls) -> Self {
        let combs_l = std::array::from_fn(|i| {
            CombFilter::new(scale_length(COMB_SIZES[i], sample_rate))
        });
        let combs_r = std::array::from_fn(|i| {
            CombFilter::new(scale_length(COMB_SIZES[i] + STEREO_SPREAD, sample_rate))
        });
        let allpasses_l = std::array::from_fn(|i| {
            AllpassFilter::new(scale_length(ALLPASS_SIZES[i], sample_rate))
        });
        let allpasses_r = std::array::from_fn(|i| {
            AllpassFilter::new(scale_length(ALLPASS_SIZES[i] + STEREO_SPREAD, sample_rate))
        });

        Self {
            ctrl: controls,
            combs_l,
            combs_r,
            allpasses_l,
            allpasses_r,
            inputs: inputs::ReverbInputs::new(),
            outputs: outputs::ReverbOutputs::new(),
            last_processed_sample: 0,
        }
    }

    /// Processes one stereo sample through the reverb.
    fn process_sample(&mut self) {
        let frozen = self.ctrl.freeze();
        let input_gain = if frozen { 0.0 } else { 1.0 };

        let feedback = if frozen {
            1.0
        } else {
            self.ctrl.room_size() * SCALE_ROOM + OFFSET_ROOM
        };
        let damping = if frozen {
            0.0
        } else {
            self.ctrl.damping() * SCALE_DAMPING
        };

        let wet_ctrl = self.ctrl.wet();
        let dry = self.ctrl.dry();
        let width = self.ctrl.width();

        let input_l = self.inputs.left();
        let input_r = self.inputs.right();
        let mixed = (input_l + input_r) * FIXED_GAIN * input_gain;

        // Parallel comb filters
        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;
        for comb in &mut self.combs_l {
            out_l += comb.tick(mixed, feedback, damping);
        }
        for comb in &mut self.combs_r {
            out_r += comb.tick(mixed, feedback, damping);
        }

        // Cascaded allpass filters
        for ap in &mut self.allpasses_l {
            out_l = ap.tick(out_l);
        }
        for ap in &mut self.allpasses_r {
            out_r = ap.tick(out_r);
        }

        // Stereo width mixing
        let wet = wet_ctrl * SCALE_WET;
        let wet_g0 = wet * (width * 0.5 + 0.5);
        let wet_g1 = wet * ((1.0 - width) * 0.5);

        let left_out = out_l * wet_g0 + out_r * wet_g1 + input_l * dry;
        let right_out = out_r * wet_g0 + out_l * wet_g1 + input_r * dry;

        self.outputs.set(left_out, right_out);
    }
}

impl Module for Reverb {
    fn name(&self) -> &str {
        "Reverb"
    }

    fn process(&mut self) -> bool {
        self.process_sample();
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
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
            ControlMeta::new("room_size", "Room size / decay length")
                .with_range(0.0, 1.0)
                .with_default(0.5),
            ControlMeta::new("damping", "High-frequency damping")
                .with_range(0.0, 1.0)
                .with_default(0.5),
            ControlMeta::new("wet", "Wet signal level")
                .with_range(0.0, 1.0)
                .with_default(0.33),
            ControlMeta::new("dry", "Dry signal level")
                .with_range(0.0, 1.0)
                .with_default(1.0),
            ControlMeta::new("width", "Stereo width")
                .with_range(0.0, 1.0)
                .with_default(1.0),
            ControlMeta::new("freeze", "Infinite hold mode")
                .with_range(0.0, 1.0)
                .with_default(0.0),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "room_size" => Ok(self.ctrl.room_size()),
            "damping" => Ok(self.ctrl.damping()),
            "wet" => Ok(self.ctrl.wet()),
            "dry" => Ok(self.ctrl.dry()),
            "width" => Ok(self.ctrl.width()),
            "freeze" => Ok(if self.ctrl.freeze() { 1.0 } else { 0.0 }),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "room_size" => self.ctrl.set_room_size(value),
            "damping" => self.ctrl.set_damping(value),
            "wet" => self.ctrl.set_wet(value),
            "dry" => self.ctrl.set_dry(value),
            "width" => self.ctrl.set_width(value),
            "freeze" => self.ctrl.set_freeze(value > 0.5),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}

/// Factory for constructing Reverb modules from configuration.
pub struct ReverbFactory;

impl ModuleFactory for ReverbFactory {
    fn type_id(&self) -> &'static str {
        "reverb"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let room_size = config
            .get("room_size")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5) as f32;
        let damping = config
            .get("damping")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5) as f32;
        let wet = config
            .get("wet")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.33) as f32;
        let dry = config
            .get("dry")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let width = config
            .get("width")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let freeze = config
            .get("freeze")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let controls = ReverbControls::new(room_size, damping, wet, dry, width, freeze);
        let reverb = Reverb::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(reverb)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_in_silence_out() {
        let mut reverb = Reverb::new(44100);
        for _ in 0..1000 {
            reverb.process();
            let l = reverb.get_output("left").unwrap();
            let r = reverb.get_output("right").unwrap();
            assert!(!l.is_nan(), "Left output is NaN");
            assert!(!r.is_nan(), "Right output is NaN");
            assert!(l.is_finite(), "Left output is infinite");
            assert!(r.is_finite(), "Right output is infinite");
        }
    }

    #[test]
    fn test_impulse_produces_reverb_tail() {
        let mut reverb = Reverb::new(44100);
        reverb.set_control("wet", 1.0).unwrap();
        reverb.set_control("dry", 0.0).unwrap();

        // Feed a single impulse
        reverb.set_input("left", 1.0).unwrap();
        reverb.process();
        reverb.set_input("left", 0.0).unwrap();

        // Check for non-zero output in the tail (after comb delay times)
        let mut found_output = false;
        for _ in 0..4000 {
            reverb.process();
            let l = reverb.get_output("left").unwrap();
            let r = reverb.get_output("right").unwrap();
            if l.abs() > 1e-6 || r.abs() > 1e-6 {
                found_output = true;
                break;
            }
        }
        assert!(found_output, "Expected reverb tail after impulse");
    }

    #[test]
    fn test_dry_passthrough() {
        let mut reverb = Reverb::new(44100);
        reverb.set_control("wet", 0.0).unwrap();
        reverb.set_control("dry", 1.0).unwrap();

        reverb.set_input("left", 0.75).unwrap();
        reverb.set_input("right", -0.5).unwrap();
        reverb.process();

        let l = reverb.get_output("left").unwrap();
        let r = reverb.get_output("right").unwrap();
        assert!(
            (l - 0.75).abs() < 1e-6,
            "Dry passthrough left: expected 0.75, got {}",
            l
        );
        assert!(
            (r - (-0.5)).abs() < 1e-6,
            "Dry passthrough right: expected -0.5, got {}",
            r
        );
    }

    #[test]
    fn test_freeze_sustains_output() {
        let mut reverb = Reverb::new(44100);
        reverb.set_control("wet", 1.0).unwrap();
        reverb.set_control("dry", 0.0).unwrap();

        // Feed signal
        for _ in 0..2000 {
            reverb.set_input("left", 0.5).unwrap();
            reverb.process();
        }
        reverb.set_input("left", 0.0).unwrap();

        // Enable freeze
        reverb.set_control("freeze", 1.0).unwrap();

        // After many samples the output should remain non-zero (frozen feedback = 1.0)
        let mut energy = 0.0f32;
        for _ in 0..4000 {
            reverb.process();
            energy += reverb.get_output("left").unwrap().abs();
        }
        assert!(
            energy > 1.0,
            "Freeze mode should sustain output, got total energy {}",
            energy
        );
    }

    #[test]
    fn test_controls() {
        let mut reverb = Reverb::new(44100);

        let controls = reverb.controls();
        assert_eq!(controls.len(), 6);
        assert_eq!(controls[0].key, "room_size");
        assert_eq!(controls[5].key, "freeze");

        reverb.set_control("room_size", 0.8).unwrap();
        assert!((reverb.get_control("room_size").unwrap() - 0.8).abs() < 1e-6);

        reverb.set_control("freeze", 1.0).unwrap();
        assert!((reverb.get_control("freeze").unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_factory() {
        let factory = ReverbFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "reverb");

        let config = serde_json::json!({
            "room_size": 0.7,
            "damping": 0.4,
            "wet": 0.5,
            "dry": 0.8,
            "freeze": false
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Reverb");
        assert_eq!(result.handles.len(), 1);
        assert_eq!(result.handles[0].0, "controls");
        assert!(result.control_surface.is_some());
        assert!(result.sink.is_none());
    }
}
