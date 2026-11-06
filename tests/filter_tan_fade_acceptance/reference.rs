//! Reference filters computed in f64 from the spec: the trapezoidal
//! state-variable filter (damping k = 2(1 - resonance), coefficient
//! g = tan(π·cutoff/sr) exactly) and the RBJ cookbook EQ biquads.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug)]
pub enum Tap {
    Low,
    High,
    Band,
}

/// One trapezoidal SVF stage.
struct Svf {
    g: f64,
    k: f64,
    ic1: f64,
    ic2: f64,
}

impl Svf {
    fn new(sr: u32, cutoff: f64, resonance: f64) -> Self {
        Self {
            g: (PI * cutoff / f64::from(sr)).tan(),
            k: 2.0 * (1.0 - resonance),
            ic1: 0.0,
            ic2: 0.0,
        }
    }

    fn tick(&mut self, v0: f64, tap: Tap) -> f64 {
        let (g, k) = (self.g, self.k);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = v0 - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match tap {
            Tap::Low => v2,
            Tap::High => v0 - k * v1 - v2,
            // The raw band tap: its peak gain at the cutoff is 1/k.
            Tap::Band => v1,
        }
    }
}

/// `input` through one SVF stage (the 12 dB slope).
pub fn svf(sr: u32, input: &[f32], tap: Tap, cutoff: f64, resonance: f64) -> Vec<f32> {
    let mut f = Svf::new(sr, cutoff, resonance);
    input
        .iter()
        .map(|&x| f.tick(f64::from(x), tap) as f32)
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub enum Eq {
    Bell,
    LowShelf,
    HighShelf,
    Notch,
}

/// `input` through the RBJ cookbook biquad (Q form), direct form I in f64.
pub fn rbj(sr: u32, input: &[f32], kind: Eq, f0: f64, db_gain: f64, q: f64) -> Vec<f32> {
    let a = 10f64.powf(db_gain / 40.0);
    let w0 = 2.0 * PI * f0 / f64::from(sr);
    let (cw, alpha) = (w0.cos(), w0.sin() / (2.0 * q));
    let sa = 2.0 * a.sqrt() * alpha;
    let (b, d) = match kind {
        Eq::Bell => (
            [1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a],
        ),
        Eq::LowShelf => (
            [
                a * ((a + 1.0) - (a - 1.0) * cw + sa),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                a * ((a + 1.0) - (a - 1.0) * cw - sa),
            ],
            [
                (a + 1.0) + (a - 1.0) * cw + sa,
                -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                (a + 1.0) + (a - 1.0) * cw - sa,
            ],
        ),
        Eq::HighShelf => (
            [
                a * ((a + 1.0) + (a - 1.0) * cw + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                a * ((a + 1.0) + (a - 1.0) * cw - sa),
            ],
            [
                (a + 1.0) - (a - 1.0) * cw + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * cw),
                (a + 1.0) - (a - 1.0) * cw - sa,
            ],
        ),
        Eq::Notch => ([1.0, -2.0 * cw, 1.0], [1.0 + alpha, -2.0 * cw, 1.0 - alpha]),
    };
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    input
        .iter()
        .map(|&x| {
            let x = f64::from(x);
            let y = (b[0] * x + b[1] * x1 + b[2] * x2 - d[1] * y1 - d[2] * y2) / d[0];
            (x2, x1, y2, y1) = (x1, x, y1, y);
            y as f32
        })
        .collect()
}
