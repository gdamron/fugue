//! A slow oscillator for vibrato and tremolo, faded in after a delay.
//!
//! Fugue's own: a sine or triangle at a rate in Hz. Each note restarts it
//! (when the patch asks), so vibrato that waits before it starts waits
//! again on every note.

use crate::dsp::sine;

/// The LFO's waveform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) enum Wave {
    Sine,
    Triangle,
}

/// The LFO, as authored in a patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) struct LfoShape {
    pub wave: Wave,
    /// Cycles per second.
    pub rate: f32,
    /// How long after a note starts it takes to fade fully in, in seconds.
    pub delay: f32,
    /// Vibrato depth, in cents either way.
    pub vibrato: f32,
    /// Tremolo depth, in dB, for operators that take it.
    pub tremolo: f32,
}

impl LfoShape {
    /// No modulation.
    pub const NONE: Self = Self {
        wave: Wave::Sine,
        rate: 5.0,
        delay: 0.0,
        vibrato: 0.0,
        tremolo: 0.0,
    };

    /// The same LFO as a triangle.
    pub const fn triangle(self) -> Self {
        Self {
            wave: Wave::Triangle,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::modules::macro_oscillator) struct Lfo {
    /// Phase in cycles.
    phase: f32,
    /// How long since the note started, in seconds.
    age: f32,
}

impl Lfo {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            age: 0.0,
        }
    }

    /// Starts again from the top, for a new note.
    pub fn restart(&mut self) {
        self.phase = 0.0;
        self.age = 0.0;
    }

    /// Moves `seconds` on and returns the LFO's value, -1 to 1, scaled by
    /// how far it has faded in.
    pub fn advance(&mut self, shape: &LfoShape, seconds: f32) -> f32 {
        self.phase += shape.rate * seconds;
        self.phase -= self.phase.floor();
        self.age = (self.age + seconds).min(1.0e4);
        let fade = if shape.delay > 0.0 {
            (self.age / shape.delay).min(1.0)
        } else {
            1.0
        };
        let value = match shape.wave {
            Wave::Sine => sine(self.phase),
            Wave::Triangle => {
                let shifted = self.phase + 0.25;
                1.0 - 4.0 * (shifted - shifted.floor() - 0.5).abs()
            }
        };
        value * fade
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(wave: Wave, delay: f32) -> LfoShape {
        LfoShape {
            wave,
            rate: 2.0,
            delay,
            vibrato: 10.0,
            tremolo: 0.0,
        }
    }

    #[test]
    fn the_waves_span_minus_one_to_one_at_the_rate() {
        for wave in [Wave::Sine, Wave::Triangle] {
            let mut lfo = Lfo::new();
            let values: Vec<f32> = (0..1_100)
                .map(|_| lfo.advance(&shape(wave, 0.0), 0.001))
                .collect();
            let (low, high) = values
                .iter()
                .fold((1.0f32, -1.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
            assert!(low < -0.99 && high > 0.99, "{wave:?}: {low} {high}");
            // Two cycles a second: crossings at 0.25, 0.5, 0.75 and 1 s.
            let crossings = values
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            assert_eq!(crossings, 4, "{wave:?}");
        }
    }

    #[test]
    fn a_delay_fades_it_in_and_a_restart_waits_again() {
        let mut lfo = Lfo::new();
        let delayed = shape(Wave::Sine, 0.5);
        let early = (0..100)
            .map(|_| lfo.advance(&delayed, 0.001).abs())
            .fold(0.0, f32::max);
        assert!(early < 0.2, "{early}");
        for _ in 0..1_000 {
            lfo.advance(&delayed, 0.001);
        }
        let late = (0..500)
            .map(|_| lfo.advance(&delayed, 0.001).abs())
            .fold(0.0, f32::max);
        assert!(late > 0.99);
        lfo.restart();
        assert!(lfo.advance(&delayed, 0.001).abs() < 0.01);
    }
}
