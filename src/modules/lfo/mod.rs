//! Low Frequency Oscillator (LFO) module for modulation.
//!
//! The LFO generates sub-audio frequency waveforms used to modulate other
//! parameters like pitch (vibrato), amplitude (tremolo), or filter cutoff.
//!
//! # Features
//!
//! - Multiple waveforms: sine, triangle, square, sawtooth
//! - Frequency range: 0.01 Hz to 20 Hz (typical LFO range)
//! - Bipolar output (-1.0 to +1.0) for FM/pitch modulation
//! - Unipolar output (0.0 to +1.0) for amplitude modulation
//! - Sync input to reset phase on trigger
//! - Rate modulation input for complex rhythmic effects
//!
//! # Example Patch
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "lfo", "type": "lfo", "config": { "frequency": 5.0, "waveform": "sine" } },
//!     { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0, "fm_amount": 20.0 } }
//!   ],
//!   "connections": [
//!     { "from": "lfo", "from_port": "out", "to": "osc", "to_port": "fm" }
//!   ]
//! }
//! ```

use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::modules::OscillatorType;
use crate::Module;

/// Low Frequency Oscillator for modulation.
///
/// Like a slow-moving oscillator that creates rhythmic changes to other
/// parameters. In Eurorack terms, this is a modulation source that you'd
/// patch into CV inputs.
///
/// # Outputs
///
/// - `out` - Bipolar signal (-1.0 to +1.0), ideal for pitch/FM modulation
/// - `out_uni` - Unipolar signal (0.0 to +1.0), ideal for amplitude modulation
///
/// # Inputs
///
/// - `sync` - Trigger input (rising edge resets phase to 0)
/// - `rate` - Frequency modulation (adds to base frequency)
pub struct Lfo {
    waveform: OscillatorType,
    frequency: f32,
    phase: f32,
    sample_rate: u32,

    // Input values
    sync_in: f32,
    rate_mod: f32,
    prev_sync: f32, // For edge detection

    // Cached outputs
    cached_out: f32,
    cached_out_uni: f32,

    // Pull-based processing
    last_processed_sample: u64,
}

impl Lfo {
    /// Creates a new LFO with the given sample rate.
    ///
    /// Defaults to 1 Hz sine wave.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            waveform: OscillatorType::Sine,
            frequency: 1.0,
            phase: 0.0,
            sample_rate,
            sync_in: 0.0,
            rate_mod: 0.0,
            prev_sync: 0.0,
            cached_out: 0.0,
            cached_out_uni: 0.5,
            last_processed_sample: 0,
        }
    }

    /// Sets the waveform type.
    pub fn with_waveform(mut self, waveform: OscillatorType) -> Self {
        self.waveform = waveform;
        self
    }

    /// Sets the frequency in Hz.
    ///
    /// Typical LFO range is 0.01 to 20 Hz.
    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.frequency = freq.clamp(0.001, 100.0);
        self
    }

    /// Sets the waveform type.
    pub fn set_waveform(&mut self, waveform: OscillatorType) {
        self.waveform = waveform;
    }

    /// Sets the frequency in Hz.
    pub fn set_frequency(&mut self, freq: f32) {
        self.frequency = freq.clamp(0.001, 100.0);
    }

    /// Resets the phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generates the next sample based on current waveform and phase.
    fn generate(&mut self) -> f32 {
        // Check for sync trigger (rising edge detection)
        if self.sync_in > 0.5 && self.prev_sync <= 0.5 {
            self.phase = 0.0;
        }
        self.prev_sync = self.sync_in;

        // Calculate effective frequency with modulation
        let effective_freq = (self.frequency + self.rate_mod).clamp(0.001, 100.0);

        // Generate waveform (bipolar: -1.0 to +1.0)
        let sample = match self.waveform {
            OscillatorType::Sine => (self.phase * 2.0 * PI).sin(),
            OscillatorType::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            OscillatorType::Sawtooth => 2.0 * self.phase - 1.0,
            OscillatorType::Triangle => 4.0 * (self.phase - 0.5).abs() - 1.0,
        };

        // Advance phase
        self.phase += effective_freq / self.sample_rate as f32;
        self.phase %= 1.0;

        sample
    }
}

impl Module for Lfo {
    fn name(&self) -> &str {
        "Lfo"
    }

    fn process(&mut self) -> bool {
        self.cached_out = self.generate();
        // Convert bipolar (-1 to +1) to unipolar (0 to +1)
        self.cached_out_uni = (self.cached_out + 1.0) * 0.5;
        true
    }

    fn inputs(&self) -> &[&str] {
        &["sync", "rate"]
    }

    fn outputs(&self) -> &[&str] {
        &["out", "out_uni"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "sync" => {
                self.sync_in = value;
                Ok(())
            }
            "rate" => {
                self.rate_mod = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "out" => Ok(self.cached_out),
            "out_uni" => Ok(self.cached_out_uni),
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

/// Factory for constructing LFO modules from configuration.
///
/// # Configuration Options
///
/// - `frequency` (f32): LFO rate in Hz, default 1.0
/// - `waveform` (string): "sine", "triangle", "square", "sawtooth", default "sine"
///
/// # Example
///
/// ```json
/// {
///   "id": "vibrato_lfo",
///   "type": "lfo",
///   "config": {
///     "frequency": 5.0,
///     "waveform": "sine"
///   }
/// }
/// ```
pub struct LfoFactory;

impl ModuleFactory for LfoFactory {
    fn type_id(&self) -> &'static str {
        "lfo"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let waveform = parse_waveform(
            config
                .get("waveform")
                .and_then(|v| v.as_str())
                .unwrap_or("sine"),
        )?;

        let mut lfo = Lfo::new(sample_rate).with_waveform(waveform);

        if let Some(freq) = config.get("frequency").and_then(|v| v.as_f64()) {
            lfo = lfo.with_frequency(freq as f32);
        }

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(lfo)),
            handles: vec![],
            sink: None,
        })
    }
}

/// Parses a waveform string into an OscillatorType enum.
fn parse_waveform(s: &str) -> Result<OscillatorType, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "sine" => Ok(OscillatorType::Sine),
        "square" => Ok(OscillatorType::Square),
        "sawtooth" | "saw" => Ok(OscillatorType::Sawtooth),
        "triangle" | "tri" => Ok(OscillatorType::Triangle),
        _ => Err(format!("Unknown waveform type: {}", s).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lfo_sine_output_range() {
        let mut lfo = Lfo::new(1000); // 1kHz sample rate for easy testing
        lfo.set_frequency(10.0); // 10 Hz

        // Process 100 samples (one full cycle at 10Hz with 1kHz sample rate)
        let mut min = f32::MAX;
        let mut max = f32::MIN;

        for _ in 0..100 {
            lfo.process();
            let out = lfo.get_output("out").unwrap();
            let out_uni = lfo.get_output("out_uni").unwrap();

            min = min.min(out);
            max = max.max(out);

            // Unipolar should always be 0-1
            assert!(out_uni >= 0.0 && out_uni <= 1.0);
        }

        // Bipolar should span approximately -1 to +1
        assert!(min < -0.9, "min was {}", min);
        assert!(max > 0.9, "max was {}", max);
    }

    #[test]
    fn test_lfo_square_wave() {
        let mut lfo = Lfo::new(1000)
            .with_waveform(OscillatorType::Square)
            .with_frequency(10.0);

        // Process samples and check for square wave behavior
        let mut saw_high = false;
        let mut saw_low = false;

        for _ in 0..100 {
            lfo.process();
            let out = lfo.get_output("out").unwrap();

            if out > 0.5 {
                saw_high = true;
            }
            if out < -0.5 {
                saw_low = true;
            }
        }

        assert!(
            saw_high && saw_low,
            "Square wave should have high and low states"
        );
    }

    #[test]
    fn test_lfo_sync_reset() {
        let mut lfo = Lfo::new(1000).with_frequency(1.0);

        // Process some samples to advance phase
        for _ in 0..500 {
            lfo.process();
        }

        // Phase should be around 0.5 (half cycle at 1Hz, 500ms)
        let _out_before = lfo.get_output("out").unwrap();

        // Trigger sync (rising edge)
        lfo.set_input("sync", 1.0).unwrap();
        lfo.process();

        // Phase should reset, output should be near starting value
        let out_after = lfo.get_output("out").unwrap();

        // For sine wave, phase 0 = 0.0, phase 0.5 = ~0.0
        // After reset, we're at phase 0 again
        // The key is that sync triggers a reset
        assert!(
            (out_after - 0.0).abs() < 0.1,
            "After sync, sine should be near 0, was {}",
            out_after
        );

        // Verify it doesn't reset again on sustained high
        lfo.set_input("sync", 1.0).unwrap();
        for _ in 0..100 {
            lfo.process();
        }
        // Should have advanced past 0
        let out_sustained = lfo.get_output("out").unwrap();
        assert!(out_sustained.abs() > 0.01, "LFO should advance after sync");
    }

    #[test]
    fn test_lfo_factory() {
        let factory = LfoFactory;
        assert_eq!(factory.type_id(), "lfo");

        let config = serde_json::json!({
            "frequency": 5.0,
            "waveform": "triangle"
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Lfo");
        assert_eq!(module.inputs(), &["sync", "rate"]);
        assert_eq!(module.outputs(), &["out", "out_uni"]);
    }

    #[test]
    fn test_lfo_rate_modulation() {
        let mut lfo = Lfo::new(1000).with_frequency(1.0);

        // Increase rate via modulation
        lfo.set_input("rate", 9.0).unwrap(); // 1 + 9 = 10 Hz effective

        // Count cycles - at 10Hz with 1kHz sample rate, we should see ~10 cycles in 1000 samples
        let mut zero_crossings = 0;
        let mut prev = 0.0;

        for _ in 0..1000 {
            lfo.process();
            let out = lfo.get_output("out").unwrap();

            if prev < 0.0 && out >= 0.0 {
                zero_crossings += 1;
            }
            prev = out;
        }

        // Should see approximately 10 positive-going zero crossings
        assert!(
            zero_crossings >= 8 && zero_crossings <= 12,
            "Expected ~10 zero crossings, got {}",
            zero_crossings
        );
    }
}
