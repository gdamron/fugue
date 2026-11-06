//! Criterion 3: the string is in tune from 55 to 1760 Hz.

use super::rig::*;

/// Octaves from 55 to 1760 Hz, then notes between them. Round octaves of 55
/// hide errors that a spectral scan on a bin-aligned grid would miss, so the
/// off-grid frequencies are the ones that make the test non-trivial.
const OCTAVES: [f32; 6] = [55.0, 110.0, 220.0, 440.0, 880.0, 1760.0];
const OFF_GRID: [f32; 6] = [65.41, 123.47, 261.63, 523.25, 987.77, 1567.98];
const TOLERANCE_CENTS: f64 = 5.0;

/// Measures `audio` over 1.5 s that start 0.2 s after the start of the sound.
fn measured(x: &[f32], hz: f32) -> f64 {
    let window = &x[seconds(0.2)..seconds(1.7)];
    assert!(peak(window) > 0.001, "string silent at {hz} Hz");
    cents(peak_near(window, f64::from(hz)), f64::from(hz))
}

/// Seconds of un-triggered string averaged at `hz`: 40 s at the lowest
/// pitch, where the resonance is closest to the noise, shortening in
/// proportion to pitch down to 3 s. The reading is then steady to about a
/// cent wherever the averaged span starts, and the test stays short.
fn welch_span(hz: f32) -> f64 {
    (2200.0 / f64::from(hz)).clamp(10.0, 40.0)
}

/// Frames in one Welch window (1.37 s), a power of two for the FFT.
const WELCH_WINDOW: usize = 1 << 16;

/// In-place radix-2 FFT of `re + i im`; `re.len()` is `WELCH_WINDOW`.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let twiddles: Vec<(f64, f64)> = (0..n / 2)
        .map(|k| (std::f64::consts::TAU * k as f64 / n as f64).sin_cos())
        .map(|(s, c)| (c, -s))
        .collect();
    let mut len = 2;
    while len <= n {
        let step = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = twiddles[k * step];
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                (re[b], im[b]) = (re[a] - xr, im[a] - xi);
                (re[a], im[a]) = (re[a] + xr, im[a] + xi);
            }
        }
        len <<= 1;
    }
}

/// Power spectrum of `x` summed (not divided; only its shape is read) over
/// Hann windows of `WELCH_WINDOW` frames that overlap by half. Two windows go
/// through one FFT (one in the real part, one in the imaginary part); the
/// sum of their powers at bin k is (|Z[k]|^2 + |Z[N-k]|^2) / 2.
fn welch_power(x: &[f32]) -> Vec<f64> {
    let n = WELCH_WINDOW;
    let hann: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let starts: Vec<usize> = (0..)
        .map(|k| k * n / 2)
        .take_while(|s| s + n <= x.len())
        .collect();
    assert!(starts.len() >= 2, "need at least two Welch windows");
    let mut power = vec![0.0; n / 2 + 1];
    let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
    for pair in starts.chunks(2) {
        for i in 0..n {
            re[i] = f64::from(x[pair[0] + i]) * hann[i];
            im[i] = pair.get(1).map_or(0.0, |s| f64::from(x[s + i]) * hann[i]);
        }
        fft(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            let m = (n - k) % n;
            *p += 0.5 * (re[k].powi(2) + im[k].powi(2) + re[m].powi(2) + im[m].powi(2));
        }
    }
    power
}

/// Frequency (Hz) of the peak of the Welch power spectrum of `x` within
/// +-50 cents of `hz`: the strongest bin, refined by a parabola through the
/// log power of that bin and its two neighbours.
fn welch_peak_near(x: &[f32], hz: f64) -> f64 {
    let power = welch_power(x);
    let bin = SRF / WELCH_WINDOW as f64;
    let at = |cents: f64| hz * 2f64.powf(cents / 1200.0) / bin;
    let top = (at(-50.0).ceil() as usize..=at(50.0).floor() as usize)
        .fold(None, |best: Option<usize>, k| match best {
            Some(b) if power[b] >= power[k] => Some(b),
            _ => Some(k),
        })
        .expect("a bin within 50 cents");
    let (below, here, above) = (power[top - 1].ln(), power[top].ln(), power[top + 1].ln());
    (top as f64 + 0.5 * (below - above) / (below - 2.0 * here + above)) * bin
}

fn assert_in_tune(hz: f32, err: f64, what: &str) {
    assert!(
        err.abs() <= TOLERANCE_CENTS,
        "string at {hz} Hz ({what}): fundamental off by {err:.2} cents"
    );
}

/// Criterion: "the string is within +-5 cents from 55 to 1760 Hz, measured on
/// `audio` at default macros", plucked by one trigger at 55-1760 Hz.
/// Assumption: the pitch is the Hann-windowed spectral peak within +-50 cents
/// of the wanted frequency, over the 1.5 s from 0.2 s after the trigger.
#[test]
fn string_pluck_is_in_tune() {
    for hz in OCTAVES.into_iter().chain(OFF_GRID) {
        let mut v = engine("string");
        v.set("frequency", hz);
        let (x, _) = v.render_driven(seconds(1.8), pulses(&[T0]));
        assert_in_tune(hz, measured(&x, hz), "pluck");
    }
}

/// Criterion: the same bound for the string with no trigger patched
/// (continuous excitation), measured on `audio` at default macros.
/// Assumption: with no trigger the string is excited by a continuous stream
/// of random impulses, which also reaches the output, so at low pitches the
/// resonance stands only about 11 dB above the noise around it and a single
/// window reads anywhere within tens of cents. The pitch is therefore the
/// peak of the power spectrum averaged over `welch_span(hz)` seconds of
/// overlapped Hann windows (`welch_peak_near`), taken from 0.5 s after the
/// start of the sound. Averaging more of the signal is the only way to make
/// this reading steady, so the criterion is read as the pitch of that
/// average, not of any 1.5 s moment.
#[test]
fn string_untriggered_is_in_tune() {
    for hz in OCTAVES.into_iter().chain(OFF_GRID) {
        let mut v = engine("string");
        v.set("frequency", hz);
        v.render(seconds(0.5));
        let x = v.render(seconds(welch_span(hz)));
        assert!(peak(&x) > 0.001, "string silent at {hz} Hz");
        let err = cents(welch_peak_near(&x, f64::from(hz)), f64::from(hz));
        assert_in_tune(hz, err, "un-triggered");
    }
}

/// Criterion: "`frequency` (Hz; overrides the control when connected)": the
/// string follows a connected frequency input to within the same bound while
/// the control sits elsewhere, plucked.
#[test]
fn string_follows_the_frequency_input() {
    for hz in OCTAVES.into_iter().chain(OFF_GRID) {
        let mut v = engine("string");
        v.set("frequency", 300.0);
        let port = v.frequency_in;
        v.hold(port, hz);
        let (x, _) = v.render_driven(seconds(1.8), pulses(&[T0]));
        assert_in_tune(hz, measured(&x, hz), "frequency input");
    }
}

/// Self-check of the measurement: a decaying 440.7 Hz sine, read the same
/// way, is within 0.2 cent, so the +-5 cent tests measure the string and not
/// the rig.
#[test]
fn measurement_reads_a_decaying_sine() {
    let hz = 440.7f64;
    let x: Vec<f32> = (0..seconds(1.8))
        .map(|i| {
            let t = i as f64 / SRF;
            ((std::f64::consts::TAU * hz * t).sin() * (-3.0 * t).exp()) as f32
        })
        .collect();
    let err = cents(peak_near(&x[seconds(0.2)..seconds(1.7)], 440.0), hz);
    assert!(err.abs() < 0.2, "rig error {err} cents");
}

/// Self-check of the averaged measurement: sine waves at off-grid
/// frequencies under broadband noise, read the same way, are within half a
/// cent, so the un-triggered test measures the string and not the estimator.
#[test]
fn averaged_measurement_reads_a_sine_in_noise() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut noise = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    };
    for hz in [55.3f64, 97.7, 130.9, 441.2, 1650.4] {
        let x: Vec<f32> = (0..seconds(8.0))
            .map(|i| {
                let t = i as f64 / SRF;
                ((std::f64::consts::TAU * hz * t).sin() + 0.35 * noise()) as f32
            })
            .collect();
        let err = cents(welch_peak_near(&x, hz * 1.01), hz);
        assert!(err.abs() < 0.5, "estimator error {err} cents at {hz} Hz");
    }
}
