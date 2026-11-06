use super::*;

const RATE: u32 = 48_000;

fn settings(dispersion: f32) -> StringSettings {
    StringSettings {
        dispersion,
        brightness: 0.5,
        damping: 0.8,
        position: 0.3,
    }
}

/// Plucks `string` with an impulse and returns `frames` of its sound.
fn pluck(string: &mut KarplusString, settings: &StringSettings, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    for b in 0..frames / 24 {
        let mut input = [0.0f32; 24];
        if b == 0 {
            input[0] = 1.0;
        }
        let (mut o, mut a) = ([0.0f32; 24], [0.0f32; 24]);
        string.process(settings, &input, &mut o, &mut a);
        out.extend_from_slice(&o);
    }
    out
}

/// The lag, in frames, at which `x` best matches itself, near `period`.
fn best_lag(x: &[f32], period: f32) -> f32 {
    let around = period.round() as usize;
    let score = |lag: usize| (0..x.len() - lag).map(|i| x[i] * x[i + lag]).sum::<f32>();
    let lag = (around - 3..=around + 3).max_by(|a, b| score(*a).total_cmp(&score(*b)));
    let lag = lag.unwrap();
    // A parabola through the three scores around the best lag.
    let (l, c, r) = (score(lag - 1), score(lag), score(lag + 1));
    lag as f32 + 0.5 * (l - r) / (l - 2.0 * c + r)
}

/// A plain string repeats at its period, the loop filter's delay included.
#[test]
fn a_plain_string_repeats_at_its_period() {
    for hz in [110.0f32, 440.0, 1760.0] {
        let mut string = KarplusString::new(RATE, false, 1);
        string.glide_to(hz / RATE as f32, 1.0);
        let x = pluck(&mut string, &settings(0.0), RATE as usize / 2);
        let period = RATE as f32 / hz;
        let lag = best_lag(&x[2400..], period);
        let cents = 1200.0 * (period / lag).log2();
        assert!(cents.abs() < 3.0, "{hz} Hz: {cents} cents");
    }
}

/// How far, in cents, the Hann-windowed spectral peak of `x` near `hz`
/// lies from it.
fn spectral_cents(x: &[f32], hz: f32) -> f32 {
    let n = x.len() as f64;
    let magnitude = |cents: f64| {
        let w = std::f64::consts::TAU * f64::from(hz) * (cents / 1200.0).exp2() / f64::from(RATE);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n).cos();
            let v = f64::from(*v) * window;
            re += v * (w * i as f64).cos();
            im -= v * (w * i as f64).sin();
        }
        re.hypot(im)
    };
    let best = |lo: f64, step: f64, steps: i32| {
        (0..=steps)
            .map(|i| lo + f64::from(i) * step)
            .max_by(|a, b| magnitude(*a).total_cmp(&magnitude(*b)))
            .unwrap()
    };
    let coarse = best(-30.0, 1.0, 60);
    best(coarse - 1.0, 0.1, 20) as f32
}

/// A loop that blocks DC holds no offset from one-signed impulses, and the
/// blocker's phase lead is taken up so the string stays in tune.
#[test]
fn a_dc_blocked_loop_holds_no_offset_and_stays_in_tune() {
    for hz in [55.0f32, 110.0, 440.0] {
        let mut string = KarplusString::new(RATE, false, 1).with_dc_blocked_loop();
        string.glide_to(hz / RATE as f32, 1.0);
        let x = pluck(&mut string, &settings(0.0), RATE as usize * 2);
        // Pitch is where the spectrum peaks: the blocker advances the phase
        // but delays the envelope, so a lag measure would read it the wrong
        // way.
        let cents = spectral_cents(&x[4800..], hz);
        assert!(cents.abs() < 2.0, "{hz} Hz: {cents} cents");

        let mut out = Vec::new();
        for b in 0..RATE as usize / 12 {
            let mut input = [0.0f32; 24];
            if b % 4 == 0 {
                input[b % 24] = 0.5;
            }
            let (mut o, mut a) = ([0.0f32; 24], [0.0f32; 24]);
            string.process(&settings(0.0), &input, &mut o, &mut a);
            out.extend_from_slice(&o);
        }
        let tail = &out[out.len() / 2..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(
            mean.abs() < 0.05 * rms,
            "{hz} Hz: mean {mean} against rms {rms}"
        );
    }
}

/// A reset string is silent at once, with no allocation needed.
#[test]
fn reset_silences_the_string() {
    for dispersion in [-0.5f32, 0.0, 0.5] {
        let mut string = KarplusString::new(RATE, true, 2);
        pluck(&mut string, &settings(dispersion), 4800);
        string.reset();
        let (mut o, mut a) = ([0.0f32; 24], [0.0f32; 24]);
        let level = string.process(&settings(dispersion), &[0.0; 24], &mut o, &mut a);
        assert_eq!(level, 0.0);
        assert!(o.iter().chain(a.iter()).all(|s| *s == 0.0));
    }
}

/// Stiffness and the buzzing bridge stay bounded and finite.
#[test]
fn every_dispersion_stays_bounded() {
    for dispersion in [-1.0f32, -0.2, 0.2, 1.0] {
        let mut string = KarplusString::new(RATE, true, 3);
        string.glide_to(55.0 / RATE as f32, 1.0);
        let x = pluck(&mut string, &settings(dispersion), RATE as usize);
        assert!(
            x.iter().all(|s| s.is_finite() && s.abs() < 4.0),
            "{dispersion}"
        );
    }
}
