//! A first-order allpass stage in topology-preserving (trapezoidal) form.
//!
//! Every frequency passes at its own level; only the phase turns, from 0 at
//! DC through −90° at the corner to −180° towards Nyquist. Chained stages
//! summed with the dry signal cancel where their phases reach −180°, −540°
//! and so on: the notches of a phaser. The corner is set per sample by a
//! coefficient, so it can sweep at audio rate without the stage going
//! unstable.
//!
//! The stage is linear in its input for a given state: its output is
//! `a·x + b` (see [`FirstOrderAllpass::response`]). A chain of them is too,
//! which lets a feedback loop around the chain be solved within the sample
//! (no one-sample delay in the loop).
//!
//! Standard technique (the bilinear-transform one-pole lowpass `lp`, with
//! `allpass = 2·lp − x`); original code.

/// One first-order allpass stage. See the [module docs](self).
#[derive(Clone, Copy, Debug, Default)]
pub struct FirstOrderAllpass {
    state: f32,
}

impl FirstOrderAllpass {
    /// A stage at rest.
    pub fn new() -> Self {
        Self::default()
    }

    /// The coefficient for a corner at `prewarped` = `tan(π·corner/rate)`:
    /// `g / (1 + g)`, from 0 (corner at DC) towards 1 (at Nyquist).
    #[inline]
    pub fn coefficient(prewarped: f32) -> f32 {
        prewarped / (1.0 + prewarped)
    }

    /// The stage's output for this sample as `(a, b)`: `a·x + b` for any
    /// input `x`, at `coefficient`. `a` depends on the coefficient only.
    #[inline]
    pub fn response(&self, coefficient: f32) -> (f32, f32) {
        (
            2.0 * coefficient - 1.0,
            2.0 * (1.0 - coefficient) * self.state,
        )
    }

    /// Runs one sample through the stage at `coefficient`.
    #[inline]
    pub fn process(&mut self, x: f32, coefficient: f32) -> f32 {
        let v = (x - self.state) * coefficient;
        let lowpass = v + self.state;
        self.state = lowpass + v;
        2.0 * lowpass - x
    }

    /// Forgets the stage's past.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn coefficient(corner: f32, rate: f32) -> f32 {
        FirstOrderAllpass::coefficient((PI * corner / rate).tan())
    }

    /// Steady-state level and phase of a sine at `frequency` through one
    /// stage cornered at `corner`.
    fn sine_through(frequency: f32, corner: f32) -> (f32, f32) {
        let rate = 48_000.0;
        let k = coefficient(corner, rate);
        let mut stage = FirstOrderAllpass::new();
        let (mut re, mut im, mut power) = (0.0f64, 0.0f64, 0.0f64);
        let n = 48_000;
        for i in 0..2 * n {
            let w = 2.0 * PI * frequency * i as f32 / rate;
            let y = stage.process(w.sin(), k);
            if i >= n {
                re += f64::from(y * w.sin());
                im += f64::from(y * w.cos());
                power += f64::from(y * y);
            }
        }
        let level = (2.0 * power / n as f64).sqrt() as f32;
        (level, im.atan2(re) as f32)
    }

    #[test]
    fn passes_every_frequency_at_its_level() {
        for frequency in [50.0, 700.0, 3000.0, 12_000.0] {
            let (level, _) = sine_through(frequency, 700.0);
            assert!((level - 1.0).abs() < 2e-3, "{frequency} Hz: {level}");
        }
    }

    #[test]
    fn turns_the_phase_a_quarter_cycle_at_the_corner() {
        let (_, phase) = sine_through(700.0, 700.0);
        assert!((phase + PI / 2.0).abs() < 0.01, "{phase}");
    }

    #[test]
    fn its_response_predicts_its_output() {
        let mut stage = FirstOrderAllpass::new();
        for i in 0..100 {
            let x = (i as f32 * 0.37).sin();
            let k = 0.1 + 0.008 * i as f32;
            let (a, b) = stage.response(k);
            let y = stage.process(x, k);
            assert!((y - (a * x + b)).abs() < 1e-6);
        }
    }
}
