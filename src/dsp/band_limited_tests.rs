use super::*;

const RATE: f32 = 48_000.0;

/// `frames` samples of an oscillator, `pick` choosing how each is made.
fn render(frames: usize, mut pick: impl FnMut(&mut BandLimitedOscillator) -> f32) -> Vec<f32> {
    let mut osc = BandLimitedOscillator::new();
    (0..frames).map(|_| pick(&mut osc)).collect()
}

/// The same waveform straight from its phase, with no smoothing.
fn naive(shape: Shape, hz: f32, width: f32, frames: usize) -> Vec<f32> {
    let increment = f64::from(hz / RATE);
    (0..frames)
        .map(|n| {
            let phase = (increment * n as f64).fract() as f32;
            value(shape, phase, width)
        })
        .collect()
}

#[cfg(feature = "spectrogram")]
mod aliasing {
    use super::*;
    use crate::dsp::RealFft;

    const SIZE: usize = 32_768;

    /// Power in the bins from 0 to 20 kHz that hold no harmonic of `hz`.
    fn alias_power(signal: &[f32], hz: f32) -> f64 {
        // Skip the start, where the oscillator's held sample is still zero.
        let windowed: Vec<f32> = signal[64..64 + SIZE]
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let t = std::f64::consts::TAU * i as f64 / SIZE as f64;
                let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()
                    - 0.01168 * (3.0 * t).cos();
                x * w as f32
            })
            .collect();
        let mut fft = RealFft::new(SIZE);
        let mut power = vec![0.0; fft.bin_count()];
        fft.power(&windowed, &mut power);
        let bin_hz = RATE / SIZE as f32;
        // Blackman-Harris leaks across about four bins either side.
        let near_harmonic = |bin: usize| {
            let f = bin as f32 * bin_hz;
            let nearest = (f / hz).round() * hz;
            (f - nearest).abs() <= 6.0 * bin_hz
        };
        (1..=(20_000.0 / bin_hz) as usize)
            .filter(|&bin| !near_harmonic(bin))
            .map(|bin| f64::from(power[bin]))
            .sum()
    }

    /// How far below the naive waveform's aliases `smooth`'s lie, in dB.
    fn reduction(smooth: &[f32], rough: &[f32], hz: f32) -> f64 {
        10.0 * (alias_power(rough, hz) / alias_power(smooth, hz)).log10()
    }

    /// Near 2 kHz, at frequencies whose aliases fall between harmonics
    /// (exactly 2 kHz at 48 kHz puts every alias on a harmonic).
    const NEAR_2K: [f32; 2] = [RATE / 23.5, RATE / 24.37];

    #[test]
    fn saw_and_pulse_aliases_are_at_least_20_db_below_naive() {
        for hz in NEAR_2K {
            for (shape, width) in [(Shape::Saw, 0.5), (Shape::Pulse, 0.5), (Shape::Pulse, 0.2)] {
                let frames = SIZE + 64;
                let smooth = render(frames, |o| o.next(shape, hz / RATE, width));
                let rough = naive(shape, hz, width, frames);
                let db = reduction(&smooth, &rough, hz);
                assert!(db >= 20.0, "{shape:?} {width} at {hz} Hz: {db:.1} dB");
            }
        }
    }

    #[test]
    fn triangle_corners_alias_less_than_naive() {
        let hz = NEAR_2K[0];
        let frames = SIZE + 64;
        let smooth = render(frames, |o| o.next(Shape::Triangle, hz / RATE, 0.5));
        let rough = naive(Shape::Triangle, hz, 0.5, frames);
        let db = reduction(&smooth, &rough, hz);
        assert!(db >= 20.0, "{db:.1} dB");
    }

    #[test]
    fn a_synced_saw_aliases_less_than_a_naive_one() {
        let (master_hz, slave_hz) = (RATE / 47.3, 1_234.5);
        let frames = SIZE + 64;
        let mut master = BandLimitedOscillator::new();
        let smooth = render(frames, |slave| {
            master.next(Shape::Saw, master_hz / RATE, 0.5);
            match master.wrapped() {
                Some(since) => slave.next_synced(Shape::Saw, slave_hz / RATE, 0.5, since),
                None => slave.next(Shape::Saw, slave_hz / RATE, 0.5),
            }
        });
        // The naive synced saw: the slave's phase restarts with the master's.
        let (mut master_phase, mut slave_phase) = (0.0f64, 0.0f64);
        let rough: Vec<f32> = (0..frames)
            .map(|_| {
                master_phase += f64::from(master_hz / RATE);
                slave_phase += f64::from(slave_hz / RATE);
                if master_phase >= 1.0 {
                    master_phase -= 1.0;
                    slave_phase = master_phase * f64::from(slave_hz / master_hz);
                }
                value(Shape::Saw, slave_phase.fract() as f32, 0.5)
            })
            .collect();
        let db = reduction(&smooth, &rough, master_hz);
        assert!(db >= 20.0, "{db:.1} dB");
    }
}

#[test]
fn away_from_edges_the_output_is_the_plain_waveform() {
    for shape in [Shape::Sine, Shape::Saw, Shape::Pulse, Shape::Triangle] {
        let hz = 110.0;
        let smooth = render(2_000, |o| o.next(shape, hz / RATE, 0.3));
        let rough = naive(shape, hz, 0.3, 2_000);
        let mut compared = 0;
        for n in 5..1_998 {
            // Skip the samples beside each jump or corner.
            let near_edge = (n - 5..n + 1).any(|k| (rough[k + 1] - rough[k]).abs() > 0.1)
                || (shape == Shape::Triangle
                    && (n - 5..n)
                        .any(|k| (rough[k + 2] - 2.0 * rough[k + 1] + rough[k]).abs() > 1e-3));
            if !near_edge {
                // The output runs two samples late, and the oscillator's first
                // sample is one increment in, where the naive one is at 0. (The
                // tolerance allows for f32 phase drift; a sample's misalignment
                // would be several times larger.)
                assert!(
                    (smooth[n] - rough[n - 1]).abs() < 1e-3,
                    "{shape:?} frame {n}"
                );
                compared += 1;
            }
        }
        assert!(compared > 1_900, "{shape:?}: {compared}");
    }
}

#[test]
fn outputs_stay_near_full_scale_at_any_pitch() {
    for shape in [Shape::Saw, Shape::Pulse, Shape::Triangle] {
        for hz in [20.0, 440.0, 5_000.0, 15_000.0, 23_000.0] {
            let out = render(4_800, |o| o.next(shape, hz / RATE, 0.5));
            let peak = out.iter().fold(0.0f32, |p, x| p.max(x.abs()));
            assert!(peak <= 1.2, "{shape:?} at {hz}: {peak}");
            let mean = out.iter().sum::<f32>() / out.len() as f32;
            assert!(mean.abs() < 0.05, "{shape:?} at {hz}: dc {mean}");
        }
    }
}

#[test]
fn a_synced_oscillator_repeats_with_its_master() {
    let master_increment = 1.0 / 96.5;
    let mut master = BandLimitedOscillator::new();
    let mut syncs = 0;
    let out = render(9_600, |slave| {
        master.next(Shape::Saw, master_increment, 0.5);
        match master.wrapped() {
            Some(since) => {
                syncs += 1;
                assert!((0.0..1.0).contains(&since));
                slave.next_synced(Shape::Pulse, 0.037, 0.5, since)
            }
            None => slave.next(Shape::Pulse, 0.037, 0.5),
        }
    });
    assert_eq!(syncs, 99);
    // Two master cycles (193 samples) apart, the slave is in the same place.
    for n in 500..9_600 - 193 {
        assert!((out[n] - out[n + 193]).abs() < 2e-2, "frame {n}");
    }
}

#[test]
fn reset_starts_the_cycle_over() {
    let mut osc = BandLimitedOscillator::new();
    let first: Vec<f32> = (0..50).map(|_| osc.next(Shape::Saw, 0.013, 0.5)).collect();
    osc.reset();
    let again: Vec<f32> = (0..50).map(|_| osc.next(Shape::Saw, 0.013, 0.5)).collect();
    assert_eq!(first, again);
}

/// The largest change between neighbouring samples.
fn largest_step(out: &[f32]) -> f32 {
    out.windows(2).fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

#[test]
fn a_width_jump_past_the_phase_is_smoothed_like_any_edge() {
    let mut osc = BandLimitedOscillator::new();
    let out: Vec<f32> = (0..1_000)
        .map(|n| {
            // At phase about 0.3, the width leaps from 0.1 to 0.9.
            let width = if n < 150 { 0.1 } else { 0.9 };
            osc.next(Shape::Pulse, 0.002, width)
        })
        .collect();
    // A hard jump would be 2; smoothed over four samples it is near 1.2.
    assert!(
        largest_step(&out[10..]) < 1.3,
        "{}",
        largest_step(&out[10..])
    );
    assert!((out[400] - 1.0).abs() < 1e-6);
}

#[test]
fn a_sync_at_the_previous_sample_is_still_smoothed() {
    let mut osc = BandLimitedOscillator::new();
    let out: Vec<f32> = (0..400)
        .map(|n| {
            if n % 97 == 50 {
                osc.next_synced(Shape::Saw, 0.03, 0.5, 1.0)
            } else {
                osc.next(Shape::Saw, 0.03, 0.5)
            }
        })
        .collect();
    assert!(
        largest_step(&out[10..]) < 1.3,
        "{}",
        largest_step(&out[10..])
    );
}

#[test]
fn inputs_that_are_not_numbers_leave_the_output_finite() {
    let mut osc = BandLimitedOscillator::new();
    for n in 0..200 {
        let odd = if n % 7 == 0 { f32::NAN } else { 0.01 };
        let x = if n % 3 == 0 {
            osc.next_synced(Shape::Pulse, odd, odd, odd)
        } else {
            osc.next(Shape::Triangle, odd, odd)
        };
        assert!(x.is_finite(), "sample {n}");
    }
}
