//! The last stage every output sink applies before its audio leaves the
//! graph: non-finite samples become silence, and hot ones are soft-clipped.
//! Shared by the DAC, the audio-file sink and the RTMP sink, so they agree
//! on what a sample becomes.

/// `sample`, or silence when it is NaN or ±inf, counted in `replaced`.
/// Branch-free, so it costs a sink loop next to nothing on finite audio.
#[inline]
pub(crate) fn finite_or_silence(sample: f32, replaced: &mut u64) -> f32 {
    let finite = sample.is_finite();
    *replaced += u64::from(!finite);
    if finite {
        sample
    } else {
        0.0
    }
}

/// Soft-clips audio that exceeds ±1.0 while preserving signals below the
/// knee. Linear below `KNEE`, then smoothly compresses toward ±1.0 using
/// `x/(1+x)` saturation on the excess. The function is C1-continuous at
/// the knee (derivative = 1.0 from both sides), so no audible artifacts
/// at the transition.
#[inline]
pub(crate) fn soft_clip(sample: f32) -> f32 {
    const KNEE: f32 = 0.95;
    const HEADROOM: f32 = 1.0 - KNEE; // 0.05

    let abs = sample.abs();
    if abs <= KNEE {
        sample
    } else {
        // Capped so a huge finite sample saturates instead of overflowing
        // to inf, where inf / inf would make it NaN.
        let excess = ((abs - KNEE) / HEADROOM).min(f32::MAX);
        let compressed = KNEE + HEADROOM * excess / (1.0 + excess);
        sample.signum() * compressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_samples_become_silence_and_are_counted() {
        let mut replaced = 0;
        let out = [f32::NAN, f32::INFINITY, -0.5, f32::NEG_INFINITY, 2.0]
            .map(|sample| finite_or_silence(sample, &mut replaced));
        assert_eq!(out, [0.0, 0.0, -0.5, 0.0, 2.0]);
        assert_eq!(replaced, 3);
    }

    #[test]
    fn a_huge_finite_sample_soft_clips_to_full_scale_not_nan() {
        for sample in [f32::MAX, -f32::MAX, 1.0e38] {
            let out = soft_clip(sample);
            assert!((out.abs() - 1.0).abs() < 1e-6, "{sample:e} -> {out}");
            assert_eq!(out.signum(), sample.signum());
        }
    }
}
