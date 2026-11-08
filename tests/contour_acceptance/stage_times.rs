//! Criterion 1: AD and AR stage times are accurate to +-1% or +-1 sample
//! (whichever is larger), from rest, in `control` range at `shape` 0.5 and
//! `smoothness` 0.5, at several frequencies and slopes.
//!
//! Measured on the gates of `envelope` output mode: `contour.2` rises at the
//! end of the attack, `contour.3` rises once the envelope has finished; both
//! are timed from the trigger (or gate edge). The envelope itself
//! (`contour.0` at `shift` 1) must peak at 1 at the same moment.
//!
//! Assumption: `contour.3` may be high at rest, so only a rise after the
//! start edge counts.

use super::rig::*;
use super::*;

const REST: usize = 1003;
const FREQS: [f32; 4] = [2.0, 5.0, 20.0, 100.0];
const SLOPES: [f32; 5] = [0.2, 0.3, 0.5, 0.75, 0.95];

/// Attack fraction of the cycle in `control` range.
fn attack_fraction(s: f64) -> f64 {
    if s >= 0.5 {
        s
    } else {
        0.5 + 0.6 * (s - 0.5) / ((s - 0.5).abs() + 0.1)
    }
}

/// (attack, decay or release) in samples, each held to at least 2.
fn stages(freq: f32, slope: f32) -> (f64, f64) {
    let t = SRF / f64::from(freq);
    let a = attack_fraction(f64::from(slope));
    ((a * t).max(2.0), ((1.0 - a) * t).max(2.0))
}

fn near(what: &str, got: usize, expected: f64) {
    let tol = (0.01 * expected).max(1.0);
    assert!(
        (got as f64 - expected).abs() <= tol,
        "{what}: {got} samples, expected {expected:.2} +-{tol:.2}"
    );
}

fn first_rise_after(x: &[f32], after: usize, what: &str) -> usize {
    (after + 1..x.len())
        .find(|&i| x[i] > 0.5 && x[i - 1] <= 0.5)
        .unwrap_or_else(|| panic!("{what}: no rising edge after sample {after}"))
}

/// Index where the envelope first reaches its peak within `from..to`.
fn peak_at(x: &[f32], from: usize, to: usize) -> (usize, f32) {
    let peak = x[from..to].iter().fold(0.0f32, |m, v| m.max(*v));
    let at = (from..to).find(|&i| x[i] >= peak - 1e-6).unwrap();
    (at, peak)
}

fn config(mode: &str, freq: f32, slope: f32) -> Value {
    json!({ "mode": mode, "output_mode": "envelope", "shift": 1.0,
            "frequency": freq, "slope": slope, "shape": 0.5, "smoothness": 0.5 })
}

#[test]
fn ad_attack_and_decay_times_are_accurate() {
    for freq in FREQS {
        for slope in SLOPES {
            let (attack, decay) = stages(freq, slope);
            let frames = REST + (attack + decay) as usize + 3000;
            let trigger = gate(frames, REST, REST + 10);
            let mut c = contour(config("ad", freq, slope));
            let out = c.render(frames, &[("trigger", &trigger[..])]);
            let what = format!("ad {freq} Hz slope {slope}");
            assert!(
                out[0][..REST].iter().all(|v| v.abs() <= 1e-6),
                "{what}: rests at 0"
            );
            let end_of_attack = first_rise_after(&out[2], REST, &what);
            let end = first_rise_after(&out[3], REST, &what);
            near(&format!("{what} attack"), end_of_attack - REST, attack);
            near(&format!("{what} decay"), end - end_of_attack, decay);
            let (top, peak) = peak_at(&out[0], REST, end);
            assert!((0.99..=1.0).contains(&peak), "{what}: peak {peak}");
            near(&format!("{what} peak time"), top - REST, attack);
            assert!(
                out[0][end + 5..].iter().all(|v| v.abs() <= 1e-3),
                "{what}: back at 0"
            );
        }
    }
}

#[test]
fn ar_attack_and_release_times_are_accurate() {
    for freq in FREQS {
        for slope in SLOPES {
            let (attack, release) = stages(freq, slope);
            let fall = REST + attack as usize + 1500;
            let frames = fall + release as usize + 3000;
            let gate_in = gate(frames, REST, fall);
            let mut c = contour(config("ar", freq, slope));
            let out = c.render(frames, &[("trigger", &gate_in[..])]);
            let what = format!("ar {freq} Hz slope {slope}");
            assert!(
                out[0][..REST].iter().all(|v| v.abs() <= 1e-6),
                "{what}: rests at 0"
            );
            near(
                &format!("{what} attack"),
                first_rise_after(&out[2], REST, &what) - REST,
                attack,
            );
            let (top, peak) = peak_at(&out[0], REST, fall);
            assert!((0.99..=1.0).contains(&peak), "{what}: peak {peak}");
            near(&format!("{what} peak time"), top - REST, attack);
            assert!(
                out[0][fall - 1] >= 0.99,
                "{what}: holds at the top while the gate is high"
            );
            let end = first_rise_after(&out[3], fall, &what);
            near(&format!("{what} release"), end - fall, release);
            assert!(
                out[0][end + 5..].iter().all(|v| v.abs() <= 1e-3),
                "{what}: back at 0"
            );
        }
    }
}
