//! "Existing LP/HP/BP render bit-identical": golden hashes of short renders
//! captured on main (11504dee) before the EQ modes landed. Each case renders
//! noise through a filter built from config, with a live `cutoff` and
//! `resonance` write halfway, then the same filter with its `cutoff` input
//! driven at audio rate.

use crate::{filter, hash, noise, sine, AMP, SR};
use serde_json::json;

/// (filter_type, resonance, golden FNV-1a hashes of the render). Captured on
/// main (11504dee): the first on macOS aarch64, the second on the Linux x86_64
/// CI runner, where libm (`log2`/`exp2` in the cutoff glide, `sin` in the
/// sweep) rounds a few results differently. Windows matches one or the other.
const GOLDEN: [(&str, f32, [u64; 2]); 6] = [
    ("lowpass", 0.5, [0x7709_c82a_f98b_50e9, 0x7709_c82a_f98b_50e9]),
    ("lowpass", 1.0, [0xa57a_e885_7610_bdd5, 0xaad9_3722_376c_2f25]),
    ("highpass", 0.5, [0x77ec_e8b0_0605_9dc0, 0x77ec_e8b0_0605_9dc0]),
    ("highpass", 1.0, [0x1bf0_350f_d7d1_7051, 0x9082_f99f_66cb_3753]),
    ("bandpass", 0.5, [0x2d9e_c36a_a2cf_dcb7, 0x8764_daef_521a_0635]),
    ("bandpass", 1.0, [0x2533_2ecd_5a3a_b705, 0x3261_2e10_4327_3c24]),
];

fn render(mode: &str, resonance: f32) -> Vec<f32> {
    let len = SR as usize / 5;
    let input = noise(len, AMP, 0x5eed_1e9a);
    let config = json!({ "filter_type": mode, "cutoff": 1200.0, "resonance": resonance });
    let mut out = filter(config.clone()).run(
        &input,
        &[(len / 2, "cutoff", 300.0), (len / 2 + 77, "resonance", 0.2)],
    );
    let sweep: Vec<f32> = (sine(len, 7.3, 1.0).iter())
        .map(|s| 2000.0 + 1800.0 * s)
        .collect();
    out.extend(filter(config).run_with(&input, &[], Some(&sweep)));
    out
}

#[test]
fn lowpass_highpass_bandpass_render_as_on_main() {
    let mut misses = Vec::new();
    for (mode, resonance, golden) in GOLDEN {
        let got = hash(&render(mode, resonance));
        if !golden.contains(&got) {
            misses.push(format!(
                "{mode} resonance {resonance}: {got:#x}, golden {golden:#x?}"
            ));
        }
    }
    assert!(misses.is_empty(), "renders changed:\n{}", misses.join("\n"));
}
