//! RBJ Audio EQ Cookbook digital biquads (Q form), as magnitude references.

use crate::SR;
use std::f64::consts::PI;

#[derive(Clone, Copy)]
pub enum Kind {
    Bell,
    LowShelf,
    HighShelf,
    Notch,
}

/// |H(e^{jw})| in dB at `freq` for the RBJ design with `f0`, `db_gain`, `q`.
pub fn magnitude_db(kind: Kind, f0: f64, db_gain: f64, q: f64, freq: f64) -> f64 {
    let a = 10f64.powf(db_gain / 40.0);
    let w0 = 2.0 * PI * f0 / f64::from(SR);
    let (cw, alpha) = (w0.cos(), w0.sin() / (2.0 * q));
    let sa = 2.0 * a.sqrt() * alpha;
    let (b, den) = match kind {
        Kind::Bell => (
            [1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a],
        ),
        Kind::LowShelf => (
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
        Kind::HighShelf => (
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
        Kind::Notch => ([1.0, -2.0 * cw, 1.0], [1.0 + alpha, -2.0 * cw, 1.0 - alpha]),
    };
    let w = 2.0 * PI * freq / f64::from(SR);
    // |c0 + c1 e^{-jw} + c2 e^{-2jw}|
    let mag = |c: [f64; 3]| {
        let re = c[0] + c[1] * w.cos() + c[2] * (2.0 * w).cos();
        let im = -c[1] * w.sin() - c[2] * (2.0 * w).sin();
        re.hypot(im)
    };
    20.0 * (mag(b) / mag(den)).log10()
}

/// A bell's band edges: the analog half-gain points
/// f0·(√(1 + 1/(4Q²)) ± 1/(2Q)).
pub fn band_edges(f0: f64, q: f64) -> (f64, f64) {
    let root = (1.0 + 1.0 / (4.0 * q * q)).sqrt();
    (f0 * (root - 1.0 / (2.0 * q)), f0 * (root + 1.0 / (2.0 * q)))
}
