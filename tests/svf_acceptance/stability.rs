use crate::{Noise, RATE};
use fugue::dsp::Svf;
use std::f32::consts::TAU;

/// Output bound for an input of at most 1: far above the resonant gain of
/// q (at most 100) but well short of a runaway filter.
const BOUND: f32 = 1e3;

fn assert_bounded(svf: &mut Svf, x: f32, what: &str) {
    let o = svf.process(x);
    for (name, v) in [
        ("lowpass", o.lowpass),
        ("bandpass", o.bandpass),
        ("highpass", o.highpass),
    ] {
        assert!(
            v.is_finite() && v.abs() < BOUND,
            "{what}: {name} {v} (cutoff {}, q {})",
            svf.cutoff(),
            svf.q()
        );
    }
}

/// Sweeping the cutoff at audio rate (the cutoff changes every sample) at high q.
#[test]
fn stays_bounded_when_the_cutoff_is_swept_at_audio_rate() {
    let rate = RATE as f32;
    for q in [10.0, 50.0, 100.0] {
        // Modulation frequencies from slow to well into the audio band.
        for lfo_hz in [5.0, 440.0, 2000.0, 9000.0] {
            for input in 0..3 {
                let mut svf = Svf::new(RATE, 1000.0, q);
                let mut noise = Noise::new(1);
                for n in 0..RATE as usize {
                    let phase = TAU * lfo_hz * n as f32 / rate;
                    // 40 Hz to 16 kHz, exponentially.
                    let fc = 40.0 * (400.0f32).powf(0.5 + 0.5 * phase.sin());
                    svf.set(fc, q);
                    let x = match input {
                        0 => noise.next() * 2.0,
                        1 => (TAU * 1000.0 * n as f32 / rate).sin(),
                        _ => (TAU * fc * n as f32 / rate).sin(),
                    };
                    assert_bounded(
                        &mut svf,
                        x,
                        &format!("q {q}, lfo {lfo_hz} Hz, input {input}, n {n}"),
                    );
                }
            }
        }
    }
}

/// The harshest sweep: the cutoff and q jump between extremes every sample.
#[test]
fn stays_bounded_when_settings_jump_between_extremes_every_sample() {
    let mut svf = Svf::new(RATE, 1000.0, 1.0);
    let mut noise = Noise::new(9);
    for n in 0..RATE as usize {
        match n % 4 {
            0 => svf.set(30.0, 100.0),
            1 => svf.set(20_000.0, 100.0),
            2 => svf.set(23_900.0, 0.1),
            _ => svf.set(400.0, 100.0),
        }
        assert_bounded(&mut svf, noise.next() * 2.0, &format!("n {n}"));
    }
}

/// Sitting at the lowest and highest q and cutoff for a long time with a loud
/// input neither blows up nor drifts.
#[test]
fn stays_bounded_at_fixed_extreme_settings() {
    for (fc, q) in [
        (30.0, 100.0),
        (23_000.0, 100.0),
        (1000.0, 100.0),
        (1000.0, 0.1),
    ] {
        let mut svf = Svf::new(RATE, fc, q);
        let mut noise = Noise::new(2);
        for n in 0..4 * RATE as usize {
            assert_bounded(
                &mut svf,
                noise.next() * 2.0,
                &format!("cutoff {fc}, q {q}, n {n}"),
            );
        }
        // And it settles back to silence once the input stops.
        for _ in 0..30 * RATE as usize {
            svf.process(0.0);
        }
        let o = svf.process(0.0);
        assert!(o.lowpass.abs() < 1e-3 && o.bandpass.abs() < 1e-3 && o.highpass.abs() < 1e-3);
    }
}
