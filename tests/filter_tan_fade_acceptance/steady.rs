//! "Steady-state output of every filter type is unchanged except for the
//! cutoff correction." With no type switch, low-cutoff renders match
//! references computed here to within -90 dBFS: the trapezoidal SVF with
//! the exact tan for lowpass / highpass / bandpass (which the old
//! approximation also matched below ~4 kHz), and the RBJ cookbook biquads
//! for the EQ types, which the cutoff fix must not touch at any frequency.
//! The 24 dB slope is not a plain cascade of the reference stage, so it is
//! held to samples captured on main (well below the -90 dBFS tolerance, the
//! old and exact tan agree at these cutoffs, and libm differences across
//! platforms are far smaller still).

// These checks are written `!(measured <= limit)` on purpose: a NaN
// measurement must count as a miss, which `measured > limit` would let pass.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use crate::reference::{rbj, svf, Eq, Tap};
use crate::{filter, max_diff, noise, render, type_position, MINUS_90_DBFS, RATES, TYPES};
use serde_json::json;

const AMP: f32 = 0.25;

fn input(sr: u32) -> Vec<f32> {
    noise(sr as usize / 5, AMP, 0x7a11_f00d)
}

#[test]
fn classic_types_at_low_cutoffs_match_the_exact_svf() {
    let taps = [
        ("lowpass", Tap::Low),
        ("highpass", Tap::High),
        ("bandpass", Tap::Band),
    ];
    let mut misses = Vec::new();
    for sr in RATES {
        let x = input(sr);
        for (mode, tap) in taps {
            for cutoff in [100.0, 1000.0, 4000.0] {
                for resonance in [0.0, 0.5, 0.9] {
                    let config = json!({
                        "filter_type": mode, "cutoff": cutoff, "resonance": resonance,
                    });
                    let got = render(sr, &config, &x);
                    let want = svf(sr, &x, tap, cutoff, resonance);
                    let diff = max_diff(&got, &want);
                    if !(diff <= MINUS_90_DBFS) {
                        misses.push(format!("{sr} Hz {config}: off by {diff:e}"));
                    }
                }
            }
        }
    }
    assert!(
        misses.is_empty(),
        "{} renders differ from the SVF reference by more than -90 dBFS:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

#[test]
fn eq_types_match_the_rbj_biquads_at_every_cutoff() {
    let cases = [
        ("bell", Eq::Bell, 1000.0, 9.0, 2.0),
        ("bell", Eq::Bell, 12000.0, -12.0, 0.7),
        (
            "lowshelf",
            Eq::LowShelf,
            300.0,
            -10.0,
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        (
            "highshelf",
            Eq::HighShelf,
            3000.0,
            6.0,
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        ("highshelf", Eq::HighShelf, 15000.0, -6.0, 1.0),
        ("notch", Eq::Notch, 1000.0, 0.0, 2.0),
    ];
    let mut misses = Vec::new();
    for sr in RATES {
        let x = input(sr);
        for (mode, kind, f0, boost, q) in cases {
            let config = json!({ "filter_type": mode, "cutoff": f0, "boost": boost, "q": q });
            let got = render(sr, &config, &x);
            let want = rbj(sr, &x, kind, f0, boost, q);
            let diff = max_diff(&got, &want);
            if !(diff <= MINUS_90_DBFS) {
                misses.push(format!("{sr} Hz {config}: off by {diff:e}"));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "{} EQ renders differ from RBJ by more than -90 dBFS:\n{}",
        misses.len(),
        misses.join("\n")
    );
}

/// A `filter_type` written before the filter is first heard lands at once:
/// no crossfade from the built type, so the render is bit-identical to a
/// filter built at the written type.
#[test]
fn type_written_before_audio_lands_at_once() {
    let sr = 48_000;
    let x = input(sr);
    for to in TYPES {
        let base = json!({ "filter_type": "lowpass", "cutoff": 1500.0, "resonance": 0.4,
                           "boost": 6.0, "q": 1.5 });
        let mut written = filter(sr, &base);
        written.set("filter_type", type_position(to));
        let got = written.run(&x, &[]);
        let mut built = base.clone();
        built["filter_type"] = json!(to);
        assert!(
            got == render(sr, &built, &x),
            "lowpass written to {to} before audio renders as built {to}"
        );
    }
}

/// (filter_type, cutoff, resonance, samples 600 + 1200·i of the 24 dB
/// render of `input(48_000)`), captured on main before the change.
const SLOPE_24: [(&str, f64, f64, [f32; 8]); 12] = [
    (
        "lowpass",
        300.0,
        0.0,
        [
            4.138868e-3,
            -1.1185369e-2,
            -9.070418e-3,
            1.5782915e-2,
            2.3476316e-3,
            2.9598275e-2,
            -8.641746e-3,
            1.7812878e-2,
        ],
    ),
    (
        "lowpass",
        300.0,
        0.9,
        [
            -2.3580173e-2,
            -1.9378575e-2,
            -3.557373e-3,
            6.29632e-2,
            -3.8721073e-2,
            3.838337e-2,
            -2.3701077e-2,
            2.720073e-2,
        ],
    ),
    (
        "lowpass",
        3000.0,
        0.0,
        [
            4.8674133e-2,
            -7.128083e-2,
            -5.0286703e-2,
            -5.6139766e-3,
            -2.4895653e-2,
            -4.3962903e-2,
            1.02196485e-2,
            -6.0007498e-2,
        ],
    ),
    (
        "lowpass",
        3000.0,
        0.9,
        [
            -4.1337226e-2,
            -3.0382017e-2,
            -5.875383e-2,
            1.830657e-2,
            1.0255102e-1,
            8.78687e-2,
            -5.748223e-2,
            -1.6992016e-1,
        ],
    ),
    (
        "highpass",
        300.0,
        0.0,
        [
            -2.5299186e-1,
            2.4642043e-1,
            1.9439578e-1,
            2.159218e-1,
            -8.8582076e-2,
            -1.11730985e-1,
            -1.0973528e-1,
            1.8864843e-1,
        ],
    ),
    (
        "highpass",
        300.0,
        0.9,
        [
            -2.8656593e-1,
            1.7193055e-1,
            1.9211242e-1,
            2.2708559e-1,
            -1.7344035e-1,
            -1.3464125e-1,
            -1.147573e-1,
            1.8707895e-1,
        ],
    ),
    (
        "highpass",
        3000.0,
        0.0,
        [
            1.7819347e-2,
            2.792338e-3,
            1.3779523e-2,
            2.3337951e-1,
            -7.960925e-2,
            -4.155627e-2,
            -8.226737e-3,
            1.582013e-2,
        ],
    ),
    (
        "highpass",
        3000.0,
        0.9,
        [
            -2.1057025e-1,
            1.4617205e-1,
            2.557471e-2,
            2.1036027e-1,
            9.85004e-2,
            -9.667158e-3,
            -5.2478593e-2,
            -8.165225e-2,
        ],
    ),
    (
        "bandpass",
        300.0,
        0.0,
        [
            1.7016683e-2,
            -4.9816226e-3,
            -6.696497e-3,
            -8.278199e-3,
            2.848213e-3,
            -1.7082557e-2,
            7.5157196e-3,
            -9.157198e-3,
        ],
    ),
    (
        "bandpass",
        300.0,
        0.9,
        [
            4.2069875e-2,
            3.401128e-2,
            -4.0496425e-3,
            -4.953371e-2,
            7.322427e-2,
            -1.5832327e-2,
            1.4909039e-2,
            -1.381509e-2,
        ],
    ),
    (
        "bandpass",
        3000.0,
        0.0,
        [
            -2.5523786e-2,
            3.7292205e-2,
            1.896574e-2,
            -8.02187e-3,
            2.608175e-3,
            -1.7932806e-2,
            -1.0185597e-2,
            3.8716815e-2,
        ],
    ),
    (
        "bandpass",
        3000.0,
        0.9,
        [
            1.2814821e-1,
            -6.3469715e-2,
            6.788399e-2,
            -6.711802e-3,
            -2.3104127e-1,
            -1.4245638e-1,
            4.3949474e-2,
            2.1371268e-1,
        ],
    ),
];

#[test]
fn slope_24db_at_low_cutoffs_renders_as_on_main() {
    let sr = 48_000;
    let x = input(sr);
    let mut misses = Vec::new();
    for (mode, cutoff, resonance, golden) in SLOPE_24 {
        let config = json!({ "filter_type": mode, "cutoff": cutoff,
                             "resonance": resonance, "slope": "24db" });
        let got = render(sr, &config, &x);
        for (i, want) in golden.into_iter().enumerate() {
            let at = 600 + 1200 * i;
            let diff = (got[at] - want).abs();
            if !(diff <= MINUS_90_DBFS) {
                misses.push(format!(
                    "{config} sample {at}: {} vs {want} on main",
                    got[at]
                ));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "24 dB renders changed:\n{}",
        misses.join("\n")
    );
}
