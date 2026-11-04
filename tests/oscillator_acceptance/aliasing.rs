use super::{alias_power, db, N, RATE, SKIP};
use fugue::dsp::{BandLimitedOscillator, Shape};

/// A saw and a square near 2 kHz alias at least 20 dB below the naive wave.
/// 48000/23.5 and 48000/24.37 are chosen so no alias lands on a harmonic.
#[test]
fn aliased_power_is_20_db_below_naive() {
    for (shape, name) in [(Shape::Saw, "saw"), (Shape::Pulse, "square")] {
        for ratio in [23.5, 24.37] {
            let inc = 1.0 / ratio;
            let f0 = RATE * inc;
            let mut osc = BandLimitedOscillator::new();
            let len = SKIP + N;
            let blep: Vec<f32> = (0..len).map(|_| osc.next(shape, inc as f32, 0.5)).collect();
            let naive: Vec<f32> = (0..len)
                .map(|n| {
                    let p = (n as f64 * inc).fract();
                    match shape {
                        Shape::Saw => 2.0 * p - 1.0,
                        _ if p < 0.5 => 1.0,
                        _ => -1.0,
                    }
                })
                .map(|v| v as f32)
                .collect();
            let ((a_blep, t_blep), (a_naive, t_naive)) =
                (alias_power(&blep, f0), alias_power(&naive, f0));
            let gain = db(a_naive / a_blep);
            println!("{name} at {f0:.2} Hz: {gain:.1} dB below naive");
            assert!(gain >= 20.0, "{name} at {f0:.2} Hz: {gain:.1} dB");
            // The wanted harmonics survive (a silent oscillator must not pass).
            // The 1.5 dB tolerance is the tester's own choice.
            assert!(db(t_blep / t_naive).abs() < 1.5, "{name} {f0:.2}: power");
        }
    }
}
