//! C5 (patches differ), C6 (brightness) and `aux` against `audio`.

use super::rig::*;

const WINDOW: usize = 1 << 14;
const CUT: f64 = 1000.0;

/// The droning patch's samples and its share of power above 1 kHz in dB.
fn droning(patch: usize, controls: &[(&str, f32)]) -> (Vec<f32>, f64) {
    let mut v = drone(patch, controls);
    let x = v.render(WINDOW);
    let share = high_share_db(&power_spectrum(&x, WINDOW), WINDOW, CUT);
    (x, share)
}

/// Criterion C5: "droning at 220 Hz, morph 0.5, no two of the 32 patches
/// render the same samples, and at least 28 of the 31 neighbouring pairs
/// differ clearly in spectrum (for example the ratio of power above 1 kHz to
/// total power differs by more than 1 dB)."
/// Assumptions: "the same samples" means every sample within 1e-6 over 16384
/// frames after 0.3 s from a fresh voice; the example measure is used as is.
#[test]
fn patches_differ_in_samples_and_neighbours_differ_in_spectrum() {
    let rendered: Vec<(Vec<f32>, f64)> = (0..32).map(|p| droning(p, &[("morph", 0.5)])).collect();
    for a in 0..32 {
        for b in a + 1..32 {
            let worst = rendered[a]
                .0
                .iter()
                .zip(&rendered[b].0)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(
                worst > 1e-6,
                "{} (patch {a}) and {} (patch {b}) render the same samples",
                PATCH_NAMES[a],
                PATCH_NAMES[b]
            );
        }
    }
    let differing: Vec<usize> = (0..31)
        .filter(|i| (rendered[*i].1 - rendered[i + 1].1).abs() > 1.0)
        .collect();
    assert!(
        differing.len() >= 28,
        "only {} of 31 neighbouring pairs differ by more than 1 dB in power above 1 kHz; \
         the shares are {:?}",
        differing.len(),
        rendered.iter().map(|r| r.1.round()).collect::<Vec<_>>()
    );
}

/// Criterion C6: "droning at 220 Hz, morph 0.5, the ratio of power above
/// 1 kHz to total power at timbre 1 exceeds the ratio at timbre 0 by at
/// least 3 dB for at least 24 of the 32 patches, and is never lower by more
/// than 1 dB."
#[test]
fn timbre_brightens_most_patches_and_never_darkens() {
    let rise: Vec<f64> = (0..32)
        .map(|p| {
            let bright = droning(p, &[("morph", 0.5), ("timbre", 1.0)]).1;
            let dark = droning(p, &[("morph", 0.5), ("timbre", 0.0)]).1;
            bright - dark
        })
        .collect();
    let brighter = rise.iter().filter(|r| **r >= 3.0).count();
    assert!(
        brighter >= 24,
        "only {brighter} of 32 patches brighten by 3 dB; rises {:?}",
        rise.iter()
            .map(|r| (r * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );
    for (p, r) in rise.iter().enumerate() {
        assert!(
            *r >= -1.0,
            "{} (patch {p}) is {:.1} dB darker at timbre 1 than at timbre 0",
            PATCH_NAMES[p],
            -r
        );
    }
}

/// Spec preamble: "`aux` is the same sound as `audio` (only the per-model
/// level may differ)". Assumption: for droning patches, `aux` equals `audio`
/// times one constant (least-squares gain, residual below 1% of audio rms).
#[test]
fn aux_is_the_audio_signal_up_to_a_fixed_level() {
    for patch in [0usize, 9, 18, 26, 31] {
        let mut v = drone(patch, &[]);
        let (audio, aux) = v.render_both(seconds(0.3));
        let gain = audio
            .iter()
            .zip(&aux)
            .map(|(a, x)| f64::from(a * x))
            .sum::<f64>()
            / audio
                .iter()
                .map(|a| f64::from(a * a))
                .sum::<f64>()
                .max(1e-30);
        assert!(gain > 0.0, "{}: aux gain {gain}", PATCH_NAMES[patch]);
        let residual: f64 = audio
            .iter()
            .zip(&aux)
            .map(|(a, x)| (f64::from(*x) - gain * f64::from(*a)).powi(2))
            .sum::<f64>()
            .sqrt();
        let energy = audio.iter().map(|a| f64::from(a * a)).sum::<f64>().sqrt();
        assert!(
            residual < 0.01 * energy,
            "{}: aux differs from audio beyond a fixed gain ({residual} vs {energy})",
            PATCH_NAMES[patch]
        );
    }
}
