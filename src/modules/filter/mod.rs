//! Filter module for subtractive synthesis.
//!
//! The filter removes or emphasizes frequencies from the input signal,
//! which is the core of subtractive synthesis. Combined with envelope
//! modulation, filters create the classic "analog synth" sound.
//!
//! # Filter Types
//!
//! - **Low-pass (LPF)** - Removes frequencies above cutoff, warm/dark sound
//! - **High-pass (HPF)** - Removes frequencies below cutoff, thin/bright sound
//! - **Band-pass (BPF)** - Passes frequencies around cutoff, vocal/nasal sound
//!
//! # Features
//!
//! - Cutoff frequency: 20 Hz to 20 kHz
//! - Resonance (Q): 0.0 to 1.0 (self-oscillates near 1.0)
//! - CV inputs for cutoff and resonance modulation
//! - State-variable filter design for stability and musicality
//!
//! # Example Patch
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc", "type": "oscillator", "config": { "oscillator_type": "sawtooth" } },
//!     { "id": "filter", "type": "filter", "config": { "filter_type": "lowpass", "cutoff": 1000.0, "resonance": 0.5 } },
//!     { "id": "env", "type": "adsr", "config": { "attack": 0.01, "decay": 0.3, "sustain": 0.2, "release": 0.5 } }
//!   ],
//!   "connections": [
//!     { "from": "osc", "from_port": "audio", "to": "filter", "to_port": "audio" },
//!     { "from": "env", "from_port": "envelope", "to": "filter", "to_port": "cutoff_cv" }
//!   ]
//! }
//! ```

use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::Module;

/// Filter type selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterType {
    /// Low-pass filter - removes frequencies above cutoff.
    #[default]
    LowPass,
    /// High-pass filter - removes frequencies below cutoff.
    HighPass,
    /// Band-pass filter - passes frequencies around cutoff.
    BandPass,
}

/// A resonant filter for subtractive synthesis.
///
/// Uses a state-variable filter (SVF) topology, which provides excellent
/// stability and can produce low-pass, high-pass, and band-pass outputs
/// simultaneously. This implementation exposes one output based on the
/// selected filter type.
///
/// # Inputs
///
/// - `audio` - Audio signal to filter
/// - `cutoff` - Base cutoff frequency in Hz (added to cutoff_cv)
/// - `cutoff_cv` - Cutoff modulation (scaled by cv_amount, in Hz)
/// - `resonance` - Resonance/Q modulation (0.0 to 1.0)
///
/// # Outputs
///
/// - `audio` - Filtered audio signal
///
/// # Example
///
/// ```rust,ignore
/// use fugue::modules::filter::{Filter, FilterType};
///
/// let mut filter = Filter::new(44100)
///     .with_filter_type(FilterType::LowPass)
///     .with_cutoff(1000.0)
///     .with_resonance(0.5);
/// ```
pub struct Filter {
    filter_type: FilterType,
    cutoff: f32,
    resonance: f32,
    cv_amount: f32,
    sample_rate: u32,

    // State-variable filter state
    low: f32,
    band: f32,

    // Input values
    audio_in: f32,
    cutoff_cv: f32,
    resonance_cv: f32,

    // Cached output
    cached_audio: f32,

    // Pull-based processing
    last_processed_sample: u64,
}

impl Filter {
    /// Creates a new filter with the given sample rate.
    ///
    /// Defaults to low-pass filter at 1000 Hz with moderate resonance.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            filter_type: FilterType::LowPass,
            cutoff: 1000.0,
            resonance: 0.0,
            cv_amount: 5000.0, // CV input scales to ±5000 Hz by default
            sample_rate,
            low: 0.0,
            band: 0.0,
            audio_in: 0.0,
            cutoff_cv: 0.0,
            resonance_cv: 0.0,
            cached_audio: 0.0,
            last_processed_sample: 0,
        }
    }

    /// Sets the filter type.
    pub fn with_filter_type(mut self, filter_type: FilterType) -> Self {
        self.filter_type = filter_type;
        self
    }

    /// Sets the cutoff frequency in Hz.
    pub fn with_cutoff(mut self, cutoff: f32) -> Self {
        self.cutoff = cutoff.clamp(20.0, 20000.0);
        self
    }

    /// Sets the resonance (Q) from 0.0 to 1.0.
    ///
    /// Higher values create more emphasis at the cutoff frequency.
    /// Values near 1.0 will cause self-oscillation.
    pub fn with_resonance(mut self, resonance: f32) -> Self {
        self.resonance = resonance.clamp(0.0, 1.0);
        self
    }

    /// Sets the CV modulation amount in Hz.
    ///
    /// This scales how much the cutoff_cv input affects the cutoff frequency.
    /// Default is 5000 Hz, meaning a CV of 1.0 adds 5000 Hz to the cutoff.
    pub fn with_cv_amount(mut self, amount: f32) -> Self {
        self.cv_amount = amount.max(0.0);
        self
    }

    /// Sets the filter type.
    pub fn set_filter_type(&mut self, filter_type: FilterType) {
        self.filter_type = filter_type;
    }

    /// Sets the cutoff frequency in Hz.
    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff.clamp(20.0, 20000.0);
    }

    /// Sets the resonance (Q) from 0.0 to 1.0.
    pub fn set_resonance(&mut self, resonance: f32) {
        self.resonance = resonance.clamp(0.0, 1.0);
    }

    /// Processes one sample through the filter.
    ///
    /// Uses a state-variable filter algorithm for stability across
    /// the full frequency range.
    fn process_sample(&mut self) -> f32 {
        // Calculate effective cutoff with CV modulation
        let effective_cutoff = (self.cutoff + self.cutoff_cv * self.cv_amount).clamp(20.0, 20000.0);

        // Calculate effective resonance with CV modulation
        let effective_resonance = (self.resonance + self.resonance_cv).clamp(0.0, 0.99);

        // Convert cutoff to filter coefficient
        // Using the formula: f = 2 * sin(pi * cutoff / sample_rate)
        // This is stable up to sample_rate/4
        let f = (2.0 * (PI * effective_cutoff / self.sample_rate as f32).sin()).min(0.99);

        // Convert resonance to Q factor
        // Q ranges from 0.5 (no resonance) to ~20 (high resonance)
        let q = 1.0 - effective_resonance;

        // State-variable filter iteration
        // This is a 2-pole (12dB/octave) filter
        self.low += f * self.band;
        let high = self.audio_in - self.low - q * self.band;
        self.band += f * high;

        // Select output based on filter type
        match self.filter_type {
            FilterType::LowPass => self.low,
            FilterType::HighPass => high,
            FilterType::BandPass => self.band,
        }
    }

    /// Resets the filter state.
    pub fn reset(&mut self) {
        self.low = 0.0;
        self.band = 0.0;
    }
}

impl Module for Filter {
    fn name(&self) -> &str {
        "Filter"
    }

    fn process(&mut self) -> bool {
        self.cached_audio = self.process_sample();
        true
    }

    fn inputs(&self) -> &[&str] {
        &["audio", "cutoff", "cutoff_cv", "resonance"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio_in = value;
                Ok(())
            }
            "cutoff" => {
                self.set_cutoff(value);
                Ok(())
            }
            "cutoff_cv" => {
                self.cutoff_cv = value;
                Ok(())
            }
            "resonance" => {
                // Can be used as direct set or CV modulation
                self.resonance_cv = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.cached_audio),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}

/// Factory for constructing Filter modules from configuration.
///
/// # Configuration Options
///
/// - `filter_type` (string): "lowpass", "highpass", "bandpass" (default: "lowpass")
/// - `cutoff` (f32): Cutoff frequency in Hz, 20-20000 (default: 1000)
/// - `resonance` (f32): Resonance/Q, 0.0-1.0 (default: 0.0)
/// - `cv_amount` (f32): CV modulation depth in Hz (default: 5000)
///
/// # Example
///
/// ```json
/// {
///   "id": "lpf",
///   "type": "filter",
///   "config": {
///     "filter_type": "lowpass",
///     "cutoff": 800.0,
///     "resonance": 0.6,
///     "cv_amount": 4000.0
///   }
/// }
/// ```
pub struct FilterFactory;

impl ModuleFactory for FilterFactory {
    fn type_id(&self) -> &'static str {
        "filter"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let filter_type = parse_filter_type(
            config
                .get("filter_type")
                .and_then(|v| v.as_str())
                .unwrap_or("lowpass"),
        )?;

        let mut filter = Filter::new(sample_rate).with_filter_type(filter_type);

        if let Some(cutoff) = config.get("cutoff").and_then(|v| v.as_f64()) {
            filter = filter.with_cutoff(cutoff as f32);
        }

        if let Some(resonance) = config.get("resonance").and_then(|v| v.as_f64()) {
            filter = filter.with_resonance(resonance as f32);
        }

        if let Some(cv_amount) = config.get("cv_amount").and_then(|v| v.as_f64()) {
            filter = filter.with_cv_amount(cv_amount as f32);
        }

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(filter)),
            handles: vec![],
            sink: None,
        })
    }
}

/// Parses a filter type string into a FilterType enum.
fn parse_filter_type(s: &str) -> Result<FilterType, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "lowpass" | "lpf" | "low" => Ok(FilterType::LowPass),
        "highpass" | "hpf" | "high" => Ok(FilterType::HighPass),
        "bandpass" | "bpf" | "band" => Ok(FilterType::BandPass),
        _ => Err(format!("Unknown filter type: {}", s).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_lowpass_attenuates_highs() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(100.0) // Very low cutoff
            .with_resonance(0.0);

        // Feed in a high-frequency signal (approximated by alternating samples)
        let mut output_sum = 0.0f32;
        for i in 0..1000 {
            // ~22kHz signal (alternating 1, -1)
            filter.audio_in = if i % 2 == 0 { 1.0 } else { -1.0 };
            filter.process();
            output_sum += filter.get_output("audio").unwrap().abs();
        }

        // Low-pass should heavily attenuate this high frequency
        let avg_output = output_sum / 1000.0;
        assert!(
            avg_output < 0.1,
            "Low-pass should attenuate high frequencies, got avg {}",
            avg_output
        );
    }

    #[test]
    fn test_filter_lowpass_passes_lows() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(5000.0)
            .with_resonance(0.0);

        // Feed in a low-frequency signal (DC offset)
        for _ in 0..1000 {
            filter.audio_in = 1.0;
            filter.process();
        }

        // After settling, output should be close to input
        let output = filter.get_output("audio").unwrap();
        assert!(
            output > 0.9,
            "Low-pass should pass DC/low frequencies, got {}",
            output
        );
    }

    #[test]
    fn test_filter_highpass_attenuates_lows() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::HighPass)
            .with_cutoff(1000.0)
            .with_resonance(0.0);

        // Feed in DC (0 Hz)
        for _ in 0..1000 {
            filter.audio_in = 1.0;
            filter.process();
        }

        // High-pass should block DC
        let output = filter.get_output("audio").unwrap();
        assert!(
            output.abs() < 0.1,
            "High-pass should attenuate DC, got {}",
            output
        );
    }

    #[test]
    fn test_filter_resonance_increases_amplitude() {
        // Test that resonance boosts signal near cutoff
        let mut filter_no_res = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(1000.0)
            .with_resonance(0.0);

        let mut filter_high_res = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(1000.0)
            .with_resonance(0.8);

        // Generate a sine wave near the cutoff frequency
        let freq = 1000.0;
        let mut max_no_res = 0.0f32;
        let mut max_high_res = 0.0f32;

        for i in 0..4410 {
            // 100ms of samples
            let t = i as f32 / 44100.0;
            let input = (2.0 * PI * freq * t).sin();

            filter_no_res.audio_in = input;
            filter_no_res.process();
            max_no_res = max_no_res.max(filter_no_res.get_output("audio").unwrap().abs());

            filter_high_res.audio_in = input;
            filter_high_res.process();
            max_high_res = max_high_res.max(filter_high_res.get_output("audio").unwrap().abs());
        }

        assert!(
            max_high_res > max_no_res,
            "Resonance should boost signal near cutoff: no_res={}, high_res={}",
            max_no_res,
            max_high_res
        );
    }

    #[test]
    fn test_filter_cutoff_cv_modulation() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(100.0)
            .with_cv_amount(5000.0);

        // With CV at 0, cutoff is 100 Hz
        filter.cutoff_cv = 0.0;

        // Feed high frequency, should be attenuated
        for _ in 0..100 {
            filter.audio_in = if filter.last_processed_sample % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            filter.process();
            filter.last_processed_sample += 1;
        }
        let output_low_cutoff = filter.get_output("audio").unwrap().abs();

        // Reset and apply CV to raise cutoff
        filter.reset();
        filter.cutoff_cv = 1.0; // +5000 Hz = 5100 Hz cutoff

        for _ in 0..100 {
            filter.audio_in = if filter.last_processed_sample % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            filter.process();
            filter.last_processed_sample += 1;
        }
        let output_high_cutoff = filter.get_output("audio").unwrap().abs();

        // Higher cutoff should let more high-frequency through
        assert!(
            output_high_cutoff > output_low_cutoff,
            "Higher cutoff should pass more signal: low={}, high={}",
            output_low_cutoff,
            output_high_cutoff
        );
    }

    #[test]
    fn test_filter_factory() {
        let factory = FilterFactory;
        assert_eq!(factory.type_id(), "filter");

        let config = serde_json::json!({
            "filter_type": "highpass",
            "cutoff": 500.0,
            "resonance": 0.3
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Filter");
        assert_eq!(
            module.inputs(),
            &["audio", "cutoff", "cutoff_cv", "resonance"]
        );
        assert_eq!(module.outputs(), &["audio"]);
    }

    #[test]
    fn test_filter_bandpass() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::BandPass)
            .with_cutoff(1000.0)
            .with_resonance(0.5);

        // Band-pass should attenuate both very low and very high frequencies
        // Test DC (should be attenuated)
        for _ in 0..1000 {
            filter.audio_in = 1.0;
            filter.process();
        }
        let dc_output = filter.get_output("audio").unwrap().abs();

        filter.reset();

        // Test very high frequency (should also be attenuated)
        let mut high_freq_sum = 0.0;
        for i in 0..1000 {
            filter.audio_in = if i % 2 == 0 { 1.0 } else { -1.0 };
            filter.process();
            high_freq_sum += filter.get_output("audio").unwrap().abs();
        }
        let high_freq_avg = high_freq_sum / 1000.0;

        assert!(
            dc_output < 0.1,
            "Band-pass should attenuate DC, got {}",
            dc_output
        );
        assert!(
            high_freq_avg < 0.3,
            "Band-pass should attenuate high frequencies, got {}",
            high_freq_avg
        );
    }

    #[test]
    fn test_filter_reset() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(1000.0);

        // Process some samples to build up state
        for _ in 0..100 {
            filter.audio_in = 1.0;
            filter.process();
        }

        let before_reset = filter.get_output("audio").unwrap();
        assert!(before_reset.abs() > 0.1, "Should have non-zero state");

        filter.reset();

        // After reset, filter should start fresh
        filter.audio_in = 0.0;
        filter.process();
        let after_reset = filter.get_output("audio").unwrap();

        assert!(
            after_reset.abs() < 0.01,
            "After reset with zero input, output should be near zero, got {}",
            after_reset
        );
    }
}
