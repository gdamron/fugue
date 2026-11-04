//! Magnitude responses of the new modes against the RBJ cookbook.

use crate::rbj::{band_edges, magnitude_db, Kind};
use crate::tone_gain_db;
use serde_json::json;

/// Checks one measured tone against an expected gain, collecting misses.
fn check(misses: &mut Vec<String>, label: String, measured: f64, expected: f64, tol: f64) {
    if !measured.is_finite() || (measured - expected).abs() > tol {
        misses.push(format!(
            "{label}: measured {measured:.3} dB, expected {expected:.3} ± {tol} dB"
        ));
    }
}

/// "Bells at ±12 dB, at 100 Hz, 1 kHz and 8 kHz, with Q 0.7, 2 and 8: within
/// ±0.5 dB at the centre and ±1 dB at the band edges." Centre is compared to
/// `boost` itself (the RBJ bell's gain at f0 is exactly dBgain); edges to the
/// RBJ digital magnitude at the analog half-gain frequencies.
#[test]
fn bells_match_rbj_at_centre_and_band_edges() {
    let mut misses = Vec::new();
    for f0 in [100.0, 1000.0, 8000.0] {
        for q in [0.7, 2.0, 8.0] {
            for boost in [12.0, -12.0] {
                let config = json!({ "filter_type": "bell", "cutoff": f0, "q": q, "boost": boost });
                let label = |at: &str| format!("bell f0 {f0} q {q} boost {boost} at {at}");
                check(
                    &mut misses,
                    label("centre"),
                    tone_gain_db(&config, f0),
                    boost,
                    0.5,
                );
                let (lo, hi) = band_edges(f0, q);
                for edge in [lo, hi] {
                    let expected = magnitude_db(Kind::Bell, f0, boost, q, edge);
                    let measured = tone_gain_db(&config, edge);
                    check(
                        &mut misses,
                        label(&format!("{edge:.1} Hz")),
                        measured,
                        expected,
                        1.0,
                    );
                }
            }
        }
    }
    assert!(
        misses.is_empty(),
        "{} misses:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

/// "Shelves are within ±0.5 dB at their plateau." Reading: the boosted side
/// sits at `boost` and the far side at 0 dB, each within ±0.5 dB, measured
/// at frequencies where the RBJ reference itself is on its plateau (checked
/// to within 0.1 dB, so the test cannot drift onto the transition). The
/// half-way point at `cutoff` is held to the RBJ value within ±1 dB.
#[test]
fn shelves_sit_on_their_plateaus() {
    // (mode, kind, cutoff, boosted-side probes, flat-side probes)
    let cases = [
        (
            "lowshelf",
            Kind::LowShelf,
            400.0,
            [20.5, 27.0],
            [9000.0, 15000.0],
        ),
        (
            "highshelf",
            Kind::HighShelf,
            2000.0,
            [16000.0, 21000.0],
            [40.0, 90.0],
        ),
    ];
    let mut misses = Vec::new();
    for (mode, kind, f0, boosted, flat) in cases {
        for q in [std::f64::consts::FRAC_1_SQRT_2, 2.0] {
            for boost in [12.0, -12.0] {
                let config = json!({ "filter_type": mode, "cutoff": f0, "q": q, "boost": boost });
                let label = |at: f64| format!("{mode} f0 {f0} q {q} boost {boost} at {at} Hz");
                for (probes, plateau) in [(boosted, boost), (flat, 0.0)] {
                    for at in probes {
                        let reference = magnitude_db(kind, f0, boost, q, at);
                        assert!(
                            (reference - plateau).abs() < 0.1,
                            "probe {at} is on the plateau"
                        );
                        check(
                            &mut misses,
                            label(at),
                            tone_gain_db(&config, at),
                            plateau,
                            0.5,
                        );
                    }
                }
                let mid = magnitude_db(kind, f0, boost, q, f0);
                check(&mut misses, label(f0), tone_gain_db(&config, f0), mid, 1.0);
            }
        }
    }
    assert!(
        misses.is_empty(),
        "{} misses:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

/// The notch "removes `cutoff`, width set by `q`": deep at the centre and on
/// the RBJ notch curve (±1 dB) at the bell-style band edges.
#[test]
fn notch_removes_its_centre() {
    let mut misses = Vec::new();
    for (f0, q) in [(1000.0, 0.7), (1000.0, 4.0), (5000.0, 2.0)] {
        let config = json!({ "filter_type": "notch", "cutoff": f0, "q": q });
        let centre = tone_gain_db(&config, f0);
        if centre.is_nan() || centre > -30.0 {
            misses.push(format!(
                "notch f0 {f0} q {q}: centre at {centre:.2} dB, want < -30"
            ));
        }
        let (lo, hi) = band_edges(f0, q);
        for edge in [lo, hi] {
            let expected = magnitude_db(Kind::Notch, f0, 0.0, q, edge);
            let label = format!("notch f0 {f0} q {q} at {edge:.1} Hz");
            check(
                &mut misses,
                label,
                tone_gain_db(&config, edge),
                expected,
                1.0,
            );
        }
    }
    assert!(
        misses.is_empty(),
        "{} misses:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

/// `slope: "24db"` "runs the signal through a second identical stage", so
/// the stopband attenuation in dB doubles; spelled `24` it is the same.
#[test]
fn slope_24db_doubles_the_attenuation() {
    for (mode, tone) in [("lowpass", 6000.0), ("highpass", 170.0)] {
        let base = json!({ "filter_type": mode, "cutoff": 1000.0, "resonance": 0.0 });
        let one = tone_gain_db(&base, tone);
        let mut steep = base.clone();
        steep["slope"] = json!("24db");
        let two = tone_gain_db(&steep, tone);
        assert!(
            one < -12.0,
            "{mode} 12db is in its stopband at {tone} Hz ({one:.2} dB)"
        );
        assert!(
            (two - 2.0 * one).abs() <= 1.0,
            "{mode} 24db at {tone} Hz: {two:.2} dB, want 2 × {one:.2} ± 1 dB"
        );
        steep["slope"] = json!("24");
        assert_eq!(
            tone_gain_db(&steep, tone),
            two,
            "{mode}: `24` spells `24db`"
        );
    }
}
