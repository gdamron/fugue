//! "LP/HP/BP place the cutoff within 0.1% of the set frequency from 20 Hz
//! to 0.45 × sample rate, at 44.1, 48 and 96 kHz."
//!
//! At resonance 0 (k = 2) the cutoff is where the lowpass and highpass
//! magnitudes are equal, so it is found by bisecting the tone frequency at
//! which the two fitted gains match. The bandpass is measured on its own:
//! its bilinear response is symmetric about the cutoff in the prewarped
//! frequency tan(πf/sr), so for a tone f1 below the peak the test bisects
//! the tone f2 above it with the same gain and reads the cutoff as
//! atan(√(tan(πf1/sr)·tan(πf2/sr))). Neither depends on the band's scaling.
//!
//! The `cutoff` control spans 20 Hz to 20 kHz, so the top point at each
//! rate is the lower of 0.45 × sr and 20 kHz.

use crate::{filter, fit, sine, RATES};
use serde_json::{json, Value};
use std::f64::consts::PI;

const TOLERANCE: f64 = 0.001;
const AMP: f32 = 0.25;
/// Tones stay below this fraction of the rate, where the fit is well posed.
const TOP_TONE: f64 = 0.48;

/// The cutoffs measured at `sr`, ending at 0.45 × sr or 20 kHz.
fn cutoffs(sr: u32) -> Vec<f64> {
    let top = (0.45 * f64::from(sr)).min(20_000.0);
    let mut points: Vec<f64> = [
        20.0, 100.0, 1000.0, 5000.0, 10_000.0, 12_000.0, 15_000.0, 18_000.0,
    ]
    .into_iter()
    .filter(|&f| f < top)
    .collect();
    points.push(top);
    points
}

/// Fitted output amplitude for a steady tone at `freq`, after a quarter
/// second of settling, over the next quarter second. With `cutoff_in`, the
/// `cutoff` input is connected and held at that value instead.
fn gain(sr: u32, config: &Value, freq: f64, cutoff_in: Option<f32>) -> f64 {
    let (skip, len) = (sr as usize / 4, sr as usize / 4);
    let input = sine(sr, skip + len, freq, AMP);
    let held = cutoff_in.map(|c| vec![c; input.len()]);
    let out = filter(sr, config).run_with(&input, &[], held.as_deref());
    let (a, b) = fit(sr, &out[skip..], skip, freq);
    a.hypot(b) / f64::from(AMP)
}

/// Bisects `f` in [lo, hi] (geometrically) for the sign change of `d`,
/// which must be positive at `lo` and negative at `hi`.
fn bisect(mut lo: f64, mut hi: f64, d: impl Fn(f64) -> f64) -> Result<f64, String> {
    let (dl, dh) = (d(lo), d(hi));
    if !(dl > 0.0 && dh < 0.0) {
        return Err(format!(
            "no crossing in [{lo:.2}, {hi:.2}] ({dl:.3e}, {dh:.3e})"
        ));
    }
    for _ in 0..22 {
        let mid = (lo * hi).sqrt();
        if d(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok((lo * hi).sqrt())
}

fn config(mode: &str, cutoff: f64) -> Value {
    json!({ "filter_type": mode, "cutoff": cutoff, "resonance": 0.0 })
}

/// The frequency where lowpass and highpass gains are equal.
fn lp_hp_cutoff(sr: u32, fc: f64, via_input: bool) -> Result<f64, String> {
    let held = via_input.then_some(fc as f32);
    // When driven through the input, the control sits elsewhere so a module
    // that ignored the input would be caught.
    let control = if via_input {
        1000.0_f64.min(fc * 0.5).max(20.0)
    } else {
        fc
    };
    let (lp, hp) = (config("lowpass", control), config("highpass", control));
    let hi = (fc * 1.1).min(TOP_TONE * f64::from(sr));
    bisect(fc * 0.9, hi, |f| {
        (gain(sr, &lp, f, held) / gain(sr, &hp, f, held)).ln()
    })
}

/// The bandpass centre from two equal-gain tones either side of it.
fn bp_cutoff(sr: u32, fc: f64) -> Result<f64, String> {
    let s = f64::from(sr);
    let t = (PI * fc / s).tan();
    let f1 = (t / 1.5).atan() * s / PI;
    let expected_f2 = (t * 1.5).atan() * s / PI;
    let bp = config("bandpass", fc);
    let target = gain(sr, &bp, f1, None);
    let lo = (fc * 0.9).max(f1 * 1.01);
    let hi = (expected_f2 * 1.1).min(TOP_TONE * s);
    let f2 = bisect(lo, hi, |f| gain(sr, &bp, f, None) - target)?;
    Ok(((PI * f1 / s).tan() * (PI * f2 / s).tan()).sqrt().atan() * s / PI)
}

fn check(misses: &mut Vec<String>, label: String, fc: f64, measured: Result<f64, String>) {
    match measured {
        Ok(m) if ((m / fc) - 1.0).abs() <= TOLERANCE => {}
        Ok(m) => misses.push(format!(
            "{label}: set {fc:.1} Hz, measured {m:.2} Hz ({:+.3}%)",
            100.0 * (m / fc - 1.0)
        )),
        Err(e) => misses.push(format!("{label}: set {fc:.1} Hz, {e}")),
    }
}

fn report(misses: Vec<String>) {
    assert!(
        misses.is_empty(),
        "{} cutoffs off by more than 0.1%:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

#[test]
fn lowpass_and_highpass_cross_at_the_set_cutoff() {
    let mut misses = Vec::new();
    for sr in RATES {
        for fc in cutoffs(sr) {
            check(
                &mut misses,
                format!("{sr} Hz LP/HP"),
                fc,
                lp_hp_cutoff(sr, fc, false),
            );
        }
    }
    report(misses);
}

#[test]
fn bandpass_peaks_at_the_set_cutoff() {
    let mut misses = Vec::new();
    for sr in RATES {
        for fc in cutoffs(sr) {
            check(&mut misses, format!("{sr} Hz BP"), fc, bp_cutoff(sr, fc));
        }
    }
    report(misses);
}

/// The `cutoff` input (Hz at audio rate) is the effective cutoff when
/// connected, so it is held to the same accuracy.
#[test]
fn cutoff_input_is_as_accurate_as_the_control() {
    let mut misses = Vec::new();
    for sr in RATES {
        for fc in cutoffs(sr).into_iter().filter(|&f| f >= 1000.0) {
            check(
                &mut misses,
                format!("{sr} Hz LP/HP via input"),
                fc,
                lp_hp_cutoff(sr, fc, true),
            );
        }
    }
    report(misses);
}
