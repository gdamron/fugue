//! "Existing LP/HP/BP render bit-identical": golden hashes of short renders
//! captured on main (11504dee) before the EQ modes landed. Each case renders
//! noise through a filter built from config, with a live `cutoff` and
//! `resonance` write halfway, then the same filter with its `cutoff` input
//! driven at audio rate.

use crate::{filter, hash, noise, sine, AMP, SR};
use serde_json::json;

/// (filter_type, resonance, golden FNV-1a hash of the render).
const GOLDEN: [(&str, f32, u64); 6] = [
    ("lowpass", 0.5, 0x7709_c82a_f98b_50e9),
    ("lowpass", 1.0, 0xa57a_e885_7610_bdd5),
    ("highpass", 0.5, 0x77ec_e8b0_0605_9dc0),
    ("highpass", 1.0, 0x1bf0_350f_d7d1_7051),
    ("bandpass", 0.5, 0x2d9e_c36a_a2cf_dcb7),
    ("bandpass", 1.0, 0x2533_2ecd_5a3a_b705),
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
        if got != golden {
            misses.push(format!(
                "{mode} resonance {resonance}: {got:#x}, golden {golden:#x}"
            ));
        }
    }
    assert!(misses.is_empty(), "renders changed:\n{}", misses.join("\n"));
}
