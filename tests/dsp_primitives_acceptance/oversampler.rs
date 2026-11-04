use crate::{amplitude, db, sine, AMP};
use fugue::dsp::{Factor, Oversampler};
use fugue::MAX_BLOCK;

const RATE: u32 = 48_000;
const SR: f64 = RATE as f64;
const FACTORS: [Factor; 2] = [Factor::Two, Factor::Four];
const SETTLE: usize = 4096; // base-rate frames discarded before measuring
const LEN: usize = 8192; // base-rate frames measured
const CHUNK: usize = 512;

/// Streams `x` (a multiple of CHUNK frames) through `upsample` or `downsample`.
fn stream(os: &mut Oversampler, x: &[f32], upward: bool) -> Vec<f32> {
    let r = os.factor().ratio();
    let (ci, co) = if upward {
        (CHUNK, CHUNK * r)
    } else {
        (CHUNK * r, CHUNK)
    };
    let mut out = vec![0.0; if upward { x.len() * r } else { x.len() / r }];
    for (i, o) in x.chunks(ci).zip(out.chunks_mut(co)) {
        match upward {
            true => os.upsample(i, o),
            false => os.downsample(i, o),
        }
    }
    out
}

/// Assumption: the round trip has unity gain, so every gain also stays within
/// 0.1 dB of 0 dB, not just within 0.1 dB of each other.
#[test]
fn passband_ripple_within_0_1_db_to_18k() {
    for factor in FACTORS {
        let tones = [100.0, 1e3, 4e3, 8e3, 12e3, 15e3, 17e3, 18e3];
        let gains = tones.map(|f| {
            let mut os = Oversampler::new(factor, RATE);
            let mut x = sine(f, SR, SETTLE + LEN);
            x.chunks_mut(CHUNK).for_each(|b| os.process(b, |_| {}));
            db(amplitude(&x[SETTLE..], f, SR) / AMP)
        });
        let hi = gains.iter().cloned().fold(f64::MIN, f64::max);
        let lo = gains.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            hi - lo <= 0.1,
            "{factor:?} ripple {} dB: {gains:?}",
            hi - lo
        );
        assert!(lo >= -0.1 && hi <= 0.1, "{factor:?} not unity: {gains:?}");
    }
}

#[test]
fn upsampling_rejects_images_by_70_db() {
    for factor in FACTORS {
        let r = factor.ratio();
        let hi_rate = SR * r as f64;
        for f in [1e3, 7e3, 12e3, 18e3] {
            let x = sine(f, SR, SETTLE + LEN);
            let y = stream(&mut Oversampler::new(factor, RATE), &x, true);
            let y = &y[SETTLE * r..];
            let tone = amplitude(y, f, hi_rate);
            // Images of an in-band tone sit at k * RATE +/- f, below the new Nyquist.
            for k in 1..=r / 2 {
                for img in [k as f64 * SR - f, k as f64 * SR + f] {
                    let rej = db(amplitude(y, img, hi_rate) / tone);
                    assert!(
                        img >= hi_rate / 2.0 || rej <= -70.0,
                        "{factor:?} {f} Hz: {img} Hz {rej} dB"
                    );
                }
            }
        }
    }
}

#[test]
fn downsampling_rejects_foldover_by_70_db() {
    for factor in FACTORS {
        let r = factor.ratio();
        let hi_rate = SR * r as f64;
        // Tones above the kept band that fold into 0..18 kHz at the base rate.
        for g in [30.5e3, 36e3, 41e3, 47e3, 53e3, 60e3, 84e3, 90e3, 95e3] {
            if g >= hi_rate / 2.0 {
                continue;
            }
            let alias = (g - (g / SR).round() * SR).abs();
            let x = sine(g, hi_rate, (SETTLE + LEN) * r);
            let y = stream(&mut Oversampler::new(factor, RATE), &x, false);
            let rej = db(amplitude(&y[SETTLE..], alias, SR) / AMP);
            assert!(
                rej <= -70.0,
                "{factor:?} {g} Hz folds to {alias} Hz: {rej} dB"
            );
        }
    }
}

/// The shaper sees `ratio * len` frames at the high rate, and `process` works in place.
#[test]
fn process_shapes_at_the_high_rate() {
    for factor in FACTORS {
        let mut os = Oversampler::new(factor, RATE);
        let mut x = sine(1000.0, SR, SETTLE + MAX_BLOCK);
        x.chunks_mut(MAX_BLOCK).for_each(|b| {
            os.process(b, |hi| {
                assert_eq!(hi.len(), MAX_BLOCK * factor.ratio());
                hi.iter_mut().for_each(|s| *s *= 0.5);
            })
        });
        let half = db(amplitude(&x[SETTLE..], 1000.0, SR) / AMP);
        assert!((half + 6.02).abs() <= 0.2, "{factor:?}: {half} dB");
    }
}
