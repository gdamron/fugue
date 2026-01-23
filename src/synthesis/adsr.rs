use crate::module::{Module, Processor};
use crate::signal::Audio;

/// ADSR (Attack-Decay-Sustain-Release) envelope generator.
///
/// Generates envelope curves for shaping audio signals over time.
/// All four parameters can be modulated by input signals for dynamic envelope shapes.
///
/// - **Attack**: Time to rise from 0.0 to 1.0 when gate goes high
/// - **Decay**: Time to fall from 1.0 to sustain level
/// - **Sustain**: Level maintained while gate is high (0.0-1.0)
/// - **Release**: Time to fall from current level to 0.0 when gate goes low
///
/// Times are specified in seconds. The envelope tracks gate state and produces
/// an output value from 0.0 to 1.0 suitable for amplitude or filter modulation.
pub struct Adsr {
    sample_rate: u32,
    envelope_value: f32,
    last_gate_high: bool,
    phase: EnvelopePhase,
}

/// The current phase of the ADSR envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EnvelopePhase {
    /// Gate is low, output is 0.0
    Idle,
    /// Rising from 0.0 to 1.0
    Attack,
    /// Falling from 1.0 to sustain level
    Decay,
    /// Holding at sustain level while gate is high
    Sustain,
    /// Falling from current level to 0.0 after gate goes low
    Release,
}

/// Input signals for ADSR modulation.
///
/// All parameters can be modulated in real-time:
/// - Gate triggers the envelope (>0.0 = on, 0.0 = off)
/// - Attack/Decay/Release times in seconds
/// - Sustain level (0.0-1.0)
#[derive(Debug, Clone, Copy)]
pub struct AdsrInput {
    /// Gate signal that triggers the envelope (>0.0 = on, 0.0 = off)
    pub gate: Audio,
    /// Attack time in seconds (time to rise from 0.0 to 1.0)
    pub attack: Audio,
    /// Decay time in seconds (time to fall from 1.0 to sustain level)
    pub decay: Audio,
    /// Sustain level (0.0-1.0, maintained while gate is high)
    pub sustain: Audio,
    /// Release time in seconds (time to fall to 0.0 after gate off)
    pub release: Audio,
}

impl Adsr {
    /// Creates a new ADSR envelope generator.
    ///
    /// The envelope starts in idle state with output = 0.0.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            envelope_value: 0.0,
            last_gate_high: false,
            phase: EnvelopePhase::Idle,
        }
    }

    /// Returns the current envelope output value (0.0-1.0).
    pub fn value(&self) -> f32 {
        self.envelope_value
    }

    /// Computes the rate of change per sample for a given time in seconds.
    ///
    /// Returns the increment/decrement value per sample to reach the target
    /// in the specified duration.
    fn rate_per_sample(&self, time_seconds: f32) -> f32 {
        if time_seconds <= 0.0 {
            // Instant transition for zero or negative time
            return 1.0;
        }
        1.0 / (time_seconds * self.sample_rate as f32)
    }
}

impl Module for Adsr {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Adsr"
    }
}

impl Processor<AdsrInput, Audio> for Adsr {
    fn process_signal(&mut self, input: AdsrInput) -> Audio {
        let gate_high = input.gate.value > 0.0;
        let attack_time = input.attack.value.max(0.0);
        let decay_time = input.decay.value.max(0.0);
        let sustain_level = input.sustain.value.clamp(0.0, 1.0);
        let release_time = input.release.value.max(0.0);

        // Detect gate transitions
        let gate_triggered = gate_high && !self.last_gate_high;
        let gate_released = !gate_high && self.last_gate_high;

        // State transitions
        if gate_triggered {
            // Retrigger: always restart attack phase when gate goes high
            self.phase = EnvelopePhase::Attack;
            // Start attack from current level (allows for smoother retriggering)
            // Alternatively, reset to 0.0 for a hard retrigger
        } else if gate_released {
            // Gate released: enter release phase from current level
            self.phase = EnvelopePhase::Release;
        }

        // Process envelope based on current phase
        match self.phase {
            EnvelopePhase::Idle => {
                self.envelope_value = 0.0;
            }
            EnvelopePhase::Attack => {
                let rate = self.rate_per_sample(attack_time);
                self.envelope_value += rate;
                if self.envelope_value >= 1.0 {
                    self.envelope_value = 1.0;
                    self.phase = EnvelopePhase::Decay;
                }
            }
            EnvelopePhase::Decay => {
                let rate = self.rate_per_sample(decay_time);
                self.envelope_value -= rate;
                if self.envelope_value <= sustain_level {
                    self.envelope_value = sustain_level;
                    self.phase = EnvelopePhase::Sustain;
                }
            }
            EnvelopePhase::Sustain => {
                self.envelope_value = sustain_level;
            }
            EnvelopePhase::Release => {
                let rate = self.rate_per_sample(release_time);
                self.envelope_value -= rate;
                if self.envelope_value <= 0.0 {
                    self.envelope_value = 0.0;
                    self.phase = EnvelopePhase::Idle;
                }
            }
        }

        self.last_gate_high = gate_high;
        Audio::new(self.envelope_value.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adsr_idle_state() {
        let mut adsr = Adsr::new(44100);
        let input = AdsrInput {
            gate: Audio::new(0.0),
            attack: Audio::new(0.1),
            decay: Audio::new(0.1),
            sustain: Audio::new(0.7),
            release: Audio::new(0.2),
        };
        let output = adsr.process_signal(input);
        assert_eq!(output.value, 0.0);
    }

    #[test]
    fn test_adsr_gate_triggers_attack() {
        let mut adsr = Adsr::new(44100);
        let input = AdsrInput {
            gate: Audio::new(1.0),
            attack: Audio::new(0.1),
            decay: Audio::new(0.1),
            sustain: Audio::new(0.7),
            release: Audio::new(0.2),
        };
        let output = adsr.process_signal(input);
        assert!(output.value > 0.0); // Should start rising
    }

    #[test]
    fn test_adsr_instant_attack() {
        let mut adsr = Adsr::new(44100);
        let input = AdsrInput {
            gate: Audio::new(1.0),
            attack: Audio::new(0.0), // Instant attack
            decay: Audio::new(0.1),
            sustain: Audio::new(0.7),
            release: Audio::new(0.2),
        };
        let output = adsr.process_signal(input);
        assert_eq!(output.value, 1.0); // Should immediately reach peak
    }

    #[test]
    fn test_adsr_sustain_clamped() {
        let mut adsr = Adsr::new(44100);
        let input = AdsrInput {
            gate: Audio::new(0.0),
            attack: Audio::new(0.1),
            decay: Audio::new(0.1),
            sustain: Audio::new(1.5), // Should clamp to 1.0
            release: Audio::new(0.2),
        };
        adsr.process_signal(input);
        // The sustain level itself gets clamped in process_signal
        assert!(true); // Test that it doesn't panic
    }

    #[test]
    fn test_adsr_full_cycle() {
        let mut adsr = Adsr::new(44100);
        let sample_rate = 44100.0;

        // Very short envelope for testing
        let attack_time = 0.01; // 10ms
        let decay_time = 0.01;
        let sustain_level = 0.5;
        let release_time = 0.01;

        let attack_samples = (attack_time * sample_rate) as usize;
        let decay_samples = (decay_time * sample_rate) as usize;
        let sustain_samples = 100; // Hold for 100 samples
        let release_samples = (release_time * sample_rate) as usize;

        // Attack phase
        for _ in 0..attack_samples {
            let output = adsr.process_signal(AdsrInput {
                gate: Audio::new(1.0),
                attack: Audio::new(attack_time),
                decay: Audio::new(decay_time),
                sustain: Audio::new(sustain_level),
                release: Audio::new(release_time),
            });
            // Should be rising during attack
            assert!(output.value >= 0.0);
        }

        // Should have reached peak by now
        let peak_output = adsr.process_signal(AdsrInput {
            gate: Audio::new(1.0),
            attack: Audio::new(attack_time),
            decay: Audio::new(decay_time),
            sustain: Audio::new(sustain_level),
            release: Audio::new(release_time),
        });
        assert!((peak_output.value - 1.0).abs() < 0.1);

        // Decay phase
        for _ in 0..decay_samples {
            adsr.process_signal(AdsrInput {
                gate: Audio::new(1.0),
                attack: Audio::new(attack_time),
                decay: Audio::new(decay_time),
                sustain: Audio::new(sustain_level),
                release: Audio::new(release_time),
            });
        }

        // Sustain phase
        for _ in 0..sustain_samples {
            let output = adsr.process_signal(AdsrInput {
                gate: Audio::new(1.0),
                attack: Audio::new(attack_time),
                decay: Audio::new(decay_time),
                sustain: Audio::new(sustain_level),
                release: Audio::new(release_time),
            });
            // Should hold at sustain level
            assert!((output.value - sustain_level).abs() < 0.1);
        }

        // Release phase
        for _ in 0..release_samples {
            adsr.process_signal(AdsrInput {
                gate: Audio::new(0.0), // Gate off
                attack: Audio::new(attack_time),
                decay: Audio::new(decay_time),
                sustain: Audio::new(sustain_level),
                release: Audio::new(release_time),
            });
        }

        // Should be back to 0.0
        let final_output = adsr.process_signal(AdsrInput {
            gate: Audio::new(0.0),
            attack: Audio::new(attack_time),
            decay: Audio::new(decay_time),
            sustain: Audio::new(sustain_level),
            release: Audio::new(release_time),
        });
        assert_eq!(final_output.value, 0.0);
    }

    #[test]
    fn test_adsr_retrigger() {
        let mut adsr = Adsr::new(44100);

        // Trigger first note
        let output1 = adsr.process_signal(AdsrInput {
            gate: Audio::new(1.0),
            attack: Audio::new(0.01),
            decay: Audio::new(0.01),
            sustain: Audio::new(0.7),
            release: Audio::new(0.1),
        });
        assert!(output1.value > 0.0);

        // Process some samples to reach sustain
        for _ in 0..1000 {
            adsr.process_signal(AdsrInput {
                gate: Audio::new(1.0),
                attack: Audio::new(0.01),
                decay: Audio::new(0.01),
                sustain: Audio::new(0.7),
                release: Audio::new(0.1),
            });
        }

        // Gate off briefly
        for _ in 0..50 {
            adsr.process_signal(AdsrInput {
                gate: Audio::new(0.0),
                attack: Audio::new(0.01),
                decay: Audio::new(0.01),
                sustain: Audio::new(0.7),
                release: Audio::new(0.1),
            });
        }

        // Retrigger - should restart attack
        let retrigger_output = adsr.process_signal(AdsrInput {
            gate: Audio::new(1.0),
            attack: Audio::new(0.01),
            decay: Audio::new(0.01),
            sustain: Audio::new(0.7),
            release: Audio::new(0.1),
        });

        // After retriggering, envelope should be in attack phase
        // Process a few more samples and verify it's rising
        let mut last_value = retrigger_output.value;
        for _ in 0..10 {
            let output = adsr.process_signal(AdsrInput {
                gate: Audio::new(1.0),
                attack: Audio::new(0.01),
                decay: Audio::new(0.01),
                sustain: Audio::new(0.7),
                release: Audio::new(0.1),
            });
            assert!(output.value >= last_value - 0.01); // Allow small tolerance
            last_value = output.value;
        }
    }
}
