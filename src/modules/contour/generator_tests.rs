use super::super::tables::tables;
use super::*;

const SR: f32 = 48_000.0;

fn params(hz: f32) -> Params {
    Params {
        frequency: hz / SR,
        shape: 0.5,
        slope: 0.5,
        smoothness: 0.5,
        shift: 0.5,
    }
}

fn modes(ramp: RampMode, output: OutputMode, range: Range) -> Modes {
    Modes {
        ramp,
        output,
        range,
    }
}

fn run(generator: &mut Generator, params: &Params, frames: usize) -> Vec<[f32; CHANNELS]> {
    (0..frames)
        .map(|_| generator.render(tables(), params, Drive::default()))
        .collect()
}

#[test]
fn phase_mode_at_the_middle_gives_four_equal_full_scale_cycles() {
    let mut g = Generator::new(modes(RampMode::Looping, OutputMode::Phase, Range::Control));
    let out = run(&mut g, &params(10.0), 9600);
    let (lo, hi) = out.iter().fold((1.0f32, -1.0f32), |(lo, hi), o| {
        (lo.min(o[0]), hi.max(o[0]))
    });
    assert!(lo < -0.99 && hi > 0.99, "{lo}..{hi}");
    for o in &out {
        assert!(o.iter().all(|v| (v - o[0]).abs() < 1e-6));
    }
}

#[test]
fn ratio_mode_at_the_top_runs_outputs_at_one_two_four_and_eight_times() {
    let mut g = Generator::new(modes(RampMode::Looping, OutputMode::Ratio, Range::Control));
    let mut p = params(2.0);
    p.shift = 1.0;
    // The new ratios take over as the first cycle ends.
    run(&mut g, &p, 24_000);
    let out = run(&mut g, &p, 48_000);
    for (j, expected) in [2, 4, 8, 16].into_iter().enumerate() {
        let rises = out
            .windows(2)
            .filter(|w| w[0][j] < 0.0 && w[1][j] >= 0.0)
            .count();
        assert!((rises as i32 - expected).abs() <= 1, "output {j}: {rises}");
    }
}

#[test]
fn envelope_gates_mark_the_end_of_attack_and_of_the_envelope() {
    let mut g = Generator::new(modes(RampMode::Ad, OutputMode::Envelope, Range::Control));
    let mut p = params(10.0);
    p.shift = 1.0;
    let trigger = Drive {
        rising: true,
        ..Drive::default()
    };
    let first = g.render(tables(), &p, trigger);
    assert_eq!(first[2], 0.0);
    let out = run(&mut g, &p, 6000);
    // 100 ms long, half of it attack.
    let attack_end = out.iter().position(|o| o[2] == 1.0).unwrap();
    let end = out.iter().position(|o| o[3] == 1.0).unwrap();
    assert!((attack_end as i32 - 2399).abs() <= 1, "{attack_end}");
    assert!((end as i32 - 4799).abs() <= 1, "{end}");
    let peak = out.iter().map(|o| o[0]).fold(0.0, f32::max);
    assert!(peak > 0.999);
}

#[test]
fn every_mode_stays_in_range_at_extreme_settings() {
    for ramp in [RampMode::Ad, RampMode::Looping, RampMode::Ar] {
        for output in [
            OutputMode::Envelope,
            OutputMode::Amplitude,
            OutputMode::Phase,
            OutputMode::Ratio,
        ] {
            for range in [Range::Control, Range::Audio] {
                let mut g = Generator::new(modes(ramp, output, range));
                for (i, (hz, x)) in [(0.01, 0.0), (5.0, 1.0), (1000.0, 0.0), (12_000.0, 1.0)]
                    .into_iter()
                    .enumerate()
                {
                    let p = Params {
                        frequency: hz / SR,
                        shape: x,
                        slope: 1.0 - x,
                        smoothness: x,
                        shift: x,
                    };
                    for k in 0..2000 {
                        let drive = Drive {
                            rising: k % 500 == 0,
                            high: (k / 300 + i) % 2 == 0,
                            clock: None,
                        };
                        for v in g.render(tables(), &p, drive) {
                            assert!(
                                v.is_finite() && v.abs() <= 1.0,
                                "{ramp:?} {output:?} {range:?} {v}"
                            );
                        }
                    }
                }
            }
        }
    }
}
