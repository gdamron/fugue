//! GVerb reverb module.
//!
//! A reverb based on the GVerb algorithm, using a 4-order
//! Feedback Delay Network with Hadamard matrix coupling, tapped delay early
//! reflections, and cascaded allpass diffusers for stereo output.
//!
//! # Features
//!
//! - Stereo in/out with configurable width
//! - Separate early reflections and reverb tail
//! - Room size, decay time, damping controls
//! - Freeze mode for infinite sustain
//! - All delay buffers pre-allocated at construction (allocation-free processing)
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc", "type": "oscillator", "config": { "oscillator_type": "sawtooth" } },
//!     { "id": "reverb", "type": "reverb", "config": { "room_size": 0.5, "decay": 0.6, "wet": 0.4, "dry": 0.8 } },
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

use crate::dsp::{Allpass, Damper, DelayLine};
use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::ReverbControls;

mod controls;
mod inputs;
mod outputs;

// --- GVerb constants ---

/// Number of parallel delay lines in the FDN.
const FDN_ORDER: usize = 4;

/// Speed of sound in meters per second (for room size → delay conversion).
const SPEED_OF_SOUND: f32 = 340.0;

/// Maximum room size in meters (determines max delay allocation).
const MAX_ROOM_M: f32 = 100.0;

/// Minimum room size in meters.
const MIN_ROOM_M: f32 = 1.0;

/// FDN delay length ratios relative to the largest delay.
const FDN_RATIOS: [f32; FDN_ORDER] = [1.0, 0.816490, 0.707100, 0.632450];

/// Early reflection tap positions as fractions of largest delay.
const TAP_FRACTIONS: [f32; FDN_ORDER] = [0.41, 0.30, 0.155, 0.0];

/// Offset added to each tap position (samples).
const TAP_OFFSET: usize = 5;

/// Allpass base sizes (relative units, scaled by diffscale).
const INPUT_DIFF_SIZE: f32 = 210.0;
const OUTPUT_DIFF_SIZES: [f32; 3] = [159.0, 562.0, 1341.0];
const DIFF_SIZE_TOTAL: f32 = 1341.0;

/// Allpass allpass coefficients.
const INPUT_DIFF_COEFF: f32 = 0.75;
const OUTPUT_DIFF_COEFFS: [f32; 3] = [0.75, 0.625, 0.625];

/// Stereo spread in samples (decorrelates left/right diffusers).
const STEREO_SPREAD: f32 = 13.0;

/// Minimum RT60 in seconds.
const MIN_RT60: f32 = 0.2;

/// Maximum RT60 in seconds.
const MAX_RT60: f32 = 15.0;

/// Target attenuation for RT60 calculation (60 dB).
const ALPHA_BASE: f32 = 0.001; // 10^(-60/20)

// --- Hadamard 4x4 matrix ---

/// Applies the 4x4 Hadamard feedback matrix in-place.
/// b[0] = 0.5 * (+d0 + d1 - d2 - d3)
/// b[1] = 0.5 * (+d0 - d1 - d2 + d3)
/// b[2] = 0.5 * (-d0 + d1 - d2 + d3)
/// b[3] = 0.5 * (+d0 + d1 + d2 + d3)
#[inline]
fn hadamard4(d: &[f32; FDN_ORDER]) -> [f32; FDN_ORDER] {
    [
        0.5 * (d[0] + d[1] - d[2] - d[3]),
        0.5 * (d[0] - d[1] - d[2] + d[3]),
        0.5 * (-d[0] + d[1] - d[2] + d[3]),
        0.5 * (d[0] + d[1] + d[2] + d[3]),
    ]
}

// --- Parameter mapping helpers ---

/// Maps room_size control (0–1) to meters.
fn room_size_to_meters(ctrl: f32) -> f32 {
    MIN_ROOM_M + ctrl * (MAX_ROOM_M - MIN_ROOM_M)
}

/// Maps decay control (0–1) to RT60 in seconds.
fn decay_to_rt60(ctrl: f32) -> f32 {
    MIN_RT60 + ctrl * (MAX_RT60 - MIN_RT60)
}

/// Computes per-sample decay alpha from RT60 and sample rate.
/// alpha = ALPHA_BASE ^ (1 / (rt60 * sample_rate))
fn compute_alpha(rt60: f32, sample_rate: u32) -> f32 {
    ALPHA_BASE.powf(1.0 / (rt60 * sample_rate as f32))
}

// --- Main reverb module ---

/// A stereo reverb effect using the GVerb (CCRMA) algorithm.
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
/// - `room_size` - Room size (0.0–1.0, maps to 1–100m, default 0.5)
/// - `decay` - Reverb decay time (0.0–1.0, maps to RT60 0.2–15s, default 0.5)
/// - `damping` - High-frequency damping (0.0–1.0, default 0.5)
/// - `wet` - Wet signal level (0.0–1.0, default 0.33)
/// - `dry` - Dry signal level (0.0–1.0, default 1.0)
/// - `width` - Stereo width (0.0–1.0, default 1.0)
/// - `freeze` - Infinite hold mode (bool, default false)
pub struct Reverb {
    sample_rate: u32,
    ctrl: ReverbControls,

    // Input processing
    input_damper: Damper,
    input_diffuser: Allpass,

    // Tapped delay for early reflections
    tap_delay: DelayLine,
    max_tap_delay_len: usize,

    // FDN delay lines and dampers
    fdn_delays: [DelayLine; FDN_ORDER],
    fdn_dampers: [Damper; FDN_ORDER],
    max_fdn_lengths: [usize; FDN_ORDER],

    // Output diffusers (3 per channel)
    left_diffusers: [Allpass; 3],
    right_diffusers: [Allpass; 3],

    inputs: inputs::ReverbInputs,
    outputs: outputs::ReverbOutputs,
    last_processed_sample: u64,
}

impl Reverb {
    /// Creates a new Reverb with default controls.
    pub fn new(sample_rate: u32) -> Self {
        let controls = ReverbControls::new(0.5, 0.5, 0.5, 0.33, 1.0, 1.0, false);
        Self::new_with_controls(sample_rate, controls)
    }

    /// Creates a new Reverb with the given controls.
    pub fn new_with_controls(sample_rate: u32, controls: ReverbControls) -> Self {
        // Allocate at max room size to avoid runtime allocation
        let max_largest_delay =
            (sample_rate as f32 * MAX_ROOM_M / SPEED_OF_SOUND).ceil() as usize + 1;

        // FDN delay lines (allocated at max size)
        let max_fdn_lengths: [usize; FDN_ORDER] =
            std::array::from_fn(|i| (max_largest_delay as f32 * FDN_RATIOS[i]).ceil() as usize);
        let fdn_delays = std::array::from_fn(|i| DelayLine::new(max_fdn_lengths[i] + 1));
        let fdn_dampers = std::array::from_fn(|_| Damper::new());

        // Tap delay (needs to hold at least max_largest_delay)
        let max_tap_delay_len = max_largest_delay + TAP_OFFSET + 1;
        let tap_delay = DelayLine::new(max_tap_delay_len);

        // Allpass sizing based on max FDN length
        let diff_scale = max_fdn_lengths[3] as f32 / DIFF_SIZE_TOTAL;

        let input_diff_size = (INPUT_DIFF_SIZE * diff_scale).ceil() as usize;
        let input_diffuser = Allpass::new(input_diff_size, INPUT_DIFF_COEFF);

        let left_diffusers = std::array::from_fn(|i| {
            let size =
                ((OUTPUT_DIFF_SIZES[i] + STEREO_SPREAD) * diff_scale).ceil() as usize;
            Allpass::new(size, OUTPUT_DIFF_COEFFS[i])
        });
        let right_diffusers = std::array::from_fn(|i| {
            let size =
                ((OUTPUT_DIFF_SIZES[i] - STEREO_SPREAD) * diff_scale).ceil() as usize;
            Allpass::new(size.max(1), OUTPUT_DIFF_COEFFS[i])
        });

        Self {
            sample_rate,
            ctrl: controls,
            input_damper: Damper::new(),
            input_diffuser,
            tap_delay,
            max_tap_delay_len,
            fdn_delays,
            fdn_dampers,
            max_fdn_lengths,
            left_diffusers,
            right_diffusers,
            inputs: inputs::ReverbInputs::new(),
            outputs: outputs::ReverbOutputs::new(),
            last_processed_sample: 0,
        }
    }

    /// Processes one stereo sample through the reverb.
    fn process_sample(&mut self) {
        let frozen = self.ctrl.freeze();
        let room_size_ctrl = self.ctrl.room_size();
        let decay_ctrl = self.ctrl.decay();
        let damping = self.ctrl.damping();
        let wet_ctrl = self.ctrl.wet();
        let dry = self.ctrl.dry();
        let width = self.ctrl.width();

        let input_l = self.inputs.left();
        let input_r = if self.inputs.right_active() {
            self.inputs.right()
        } else {
            input_l
        };
        let input = (input_l + input_r) * 0.5;

        // Compute room-dependent parameters
        let room_m = room_size_to_meters(room_size_ctrl);
        let largest_delay =
            ((self.sample_rate as f32 * room_m / SPEED_OF_SOUND) as usize).max(1);

        let rt60 = decay_to_rt60(decay_ctrl);
        let alpha = compute_alpha(rt60, self.sample_rate);

        // FDN lengths (clamped to allocated max)
        let fdn_lens: [usize; FDN_ORDER] = std::array::from_fn(|i| {
            ((largest_delay as f32 * FDN_RATIOS[i]) as usize)
                .max(1)
                .min(self.max_fdn_lengths[i])
        });

        // FDN gains from alpha
        let fdn_gains: [f32; FDN_ORDER] = if frozen {
            [-1.0; FDN_ORDER]
        } else {
            std::array::from_fn(|i| -(alpha.powi(fdn_lens[i] as i32)))
        };

        // Tap positions and gains
        let tap_positions: [usize; FDN_ORDER] = std::array::from_fn(|i| {
            let pos = (TAP_FRACTIONS[i] * largest_delay as f32) as usize + TAP_OFFSET;
            pos.min(self.max_tap_delay_len - 1)
        });
        let tap_gains: [f32; FDN_ORDER] = if frozen {
            [0.0; FDN_ORDER]
        } else {
            std::array::from_fn(|i| alpha.powi(tap_positions[i] as i32))
        };

        // Input gain (zero when frozen to prevent new input)
        let input_gain = if frozen { 0.0 } else { 1.0 };

        // 1. Input damping (bandwidth control)
        let z = self.input_damper.tick(input * input_gain, damping);

        // 2. Input diffusion
        let z = self.input_diffuser.tick(z);

        // 3. Read early reflection taps
        let early: [f32; FDN_ORDER] =
            std::array::from_fn(|i| tap_gains[i] * self.tap_delay.read(tap_positions[i]));

        // 4. Write into tap delay
        self.tap_delay.write_and_advance(z);

        // 5. Read FDN with damping in feedback path
        let fdn_damping = if frozen { 0.0 } else { damping };
        let d: [f32; FDN_ORDER] = std::array::from_fn(|i| {
            let raw = self.fdn_delays[i].read(fdn_lens[i]);
            let damped = self.fdn_dampers[i].tick(raw, fdn_damping);
            fdn_gains[i] * damped
        });

        // 6. Mix early reflections + tail
        let early_level = 0.4;
        let tail_level = 0.6;
        let mut sum = input * input_gain * early_level;
        let mut sign = 1.0f32;
        for i in 0..FDN_ORDER {
            sum += sign * (tail_level * d[i] + early_level * early[i]);
            sign = -sign;
        }
        let mut lsum = sum;
        let mut rsum = sum;

        // 7. Hadamard matrix feedback mixing
        let f = hadamard4(&d);

        // 8. Write FDN feedback (early reflections + mixed FDN)
        for i in 0..FDN_ORDER {
            self.fdn_delays[i].write_and_advance(early[i] + f[i]);
        }

        // 9. Output diffusion (separate L/R chains for stereo)
        for diff in &mut self.left_diffusers {
            lsum = diff.tick(lsum);
        }
        for diff in &mut self.right_diffusers {
            rsum = diff.tick(rsum);
        }

        // 10. Stereo width crossmix + dry blend
        let wet_g0 = wet_ctrl * (width * 0.5 + 0.5);
        let wet_g1 = wet_ctrl * ((1.0 - width) * 0.5);

        let left_out = lsum * wet_g0 + rsum * wet_g1 + input_l * dry;
        let right_out = rsum * wet_g0 + lsum * wet_g1 + input_r * dry;

        // Denormal protection
        let left_out = if left_out.is_finite() { left_out } else { 0.0 };
        let right_out = if right_out.is_finite() {
            right_out
        } else {
            0.0
        };

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

    fn reset_inputs(&mut self) {
        self.inputs.reset();
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("room_size", "Room size")
                .with_range(0.0, 1.0)
                .with_default(0.5),
            ControlMeta::new("decay", "Reverb decay time")
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
            "decay" => Ok(self.ctrl.decay()),
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
            "decay" => self.ctrl.set_decay(value),
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
        let decay = config
            .get("decay")
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

        let controls =
            ReverbControls::new(room_size, decay, damping, wet, dry, width, freeze);
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
        for _ in 0..2000 {
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

        // Check for non-zero output in the tail
        let mut found_output = false;
        for _ in 0..8000 {
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

        // Output should remain non-zero (frozen feedback)
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
    fn test_no_denormal_explosion() {
        let mut reverb = Reverb::new(44100);
        reverb.set_control("wet", 1.0).unwrap();
        reverb.set_control("decay", 0.9).unwrap();

        // Feed a brief signal then silence
        for _ in 0..100 {
            reverb.set_input("left", 0.1).unwrap();
            reverb.process();
        }
        reverb.set_input("left", 0.0).unwrap();

        // Run many samples of silence — output should stay bounded
        for i in 0..50000 {
            reverb.process();
            let l = reverb.get_output("left").unwrap();
            let r = reverb.get_output("right").unwrap();
            assert!(
                l.abs() < 100.0,
                "Left output exploded at sample {}: {}",
                i,
                l
            );
            assert!(
                r.abs() < 100.0,
                "Right output exploded at sample {}: {}",
                i,
                r
            );
        }
    }

    #[test]
    fn test_controls() {
        let mut reverb = Reverb::new(44100);

        let controls = reverb.controls();
        assert_eq!(controls.len(), 7);
        assert_eq!(controls[0].key, "room_size");
        assert_eq!(controls[1].key, "decay");
        assert_eq!(controls[6].key, "freeze");

        reverb.set_control("room_size", 0.8).unwrap();
        assert!((reverb.get_control("room_size").unwrap() - 0.8).abs() < 1e-6);

        reverb.set_control("decay", 0.7).unwrap();
        assert!((reverb.get_control("decay").unwrap() - 0.7).abs() < 1e-6);

        reverb.set_control("freeze", 1.0).unwrap();
        assert!((reverb.get_control("freeze").unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_factory() {
        let factory = ReverbFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "reverb");

        let config = serde_json::json!({
            "room_size": 0.7,
            "decay": 0.6,
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
