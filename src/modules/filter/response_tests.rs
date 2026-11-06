//! `filter_type` switches crossfade.

use std::f32::consts::TAU;

use super::*;

const RATE: u32 = 48_000;

/// A 220 Hz saw, band-limited to 20 harmonics.
fn saw(n: usize) -> f32 {
    let phase = TAU * 220.0 * n as f32 / RATE as f32;
    0.3 * (1..=20)
        .map(|h| (phase * h as f32).sin() / h as f32)
        .sum::<f32>()
}

/// Runs `filter` over the saw from sample `from`, one sample at a time.
fn run(filter: &mut Filter, from: usize, frames: usize) -> Vec<f32> {
    (from..from + frames)
        .map(|n| {
            filter.set_input("audio", saw(n)).unwrap();
            filter.process(1);
            filter.get_output("audio").unwrap()
        })
        .collect()
}

fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

fn fade_frames() -> usize {
    (RATE as f32 * FADE_MS / 1000.0).round() as usize
}

#[test]
fn a_type_switch_crossfades_without_a_step() {
    use FilterType::*;
    let switches = [
        (LowPass, HighPass),
        (HighPass, BandPass),
        (BandPass, LowPass),
        (LowPass, Notch),
        (Bell, HighPass),
    ];
    for (from, to) in switches {
        let mut filter = Filter::new(RATE)
            .with_filter_type(from)
            .with_cutoff(1000.0)
            .with_resonance(0.3);
        filter.set_control("boost", 9.0).unwrap();
        let before = run(&mut filter, 0, 24_000);
        filter.set_filter_type(to);
        let across = run(&mut filter, 24_000, 2 * fade_frames());
        let after = run(&mut filter, 24_000 + 2 * fade_frames(), 24_000);
        let steady = largest_step(&before[12_000..]).max(largest_step(&after[12_000..]));
        let mut joined = vec![before[before.len() - 1]];
        joined.extend(across);
        let step = largest_step(&joined);
        assert!(
            step <= 1.5 * steady,
            "{from:?} -> {to:?}: {step} vs {steady}"
        );
    }
}

#[test]
fn a_type_switch_takes_the_fade_time() {
    let mut filter = Filter::new(RATE).with_cutoff(1000.0);
    run(&mut filter, 0, 1_000);
    filter.set_filter_type(FilterType::HighPass);
    run(&mut filter, 1_000, fade_frames() - 1);
    assert!(filter.type_fade.is_fading());
    run(&mut filter, 1_000 + fade_frames() - 1, 1);
    assert!(!filter.type_fade.is_fading());
}

#[test]
fn a_switch_during_a_fade_waits_for_it() {
    let mut filter = Filter::new(RATE).with_cutoff(1000.0);
    let before = run(&mut filter, 0, 24_000);
    filter.set_filter_type(FilterType::HighPass);
    let mut out = run(&mut filter, 24_000, 100);
    filter.set_filter_type(FilterType::BandPass);
    out.extend(run(&mut filter, 24_100, fade_frames() - 100));
    assert_eq!(filter.playing.filter_type, FilterType::HighPass);
    out.extend(run(&mut filter, 24_000 + fade_frames(), 1));
    assert_eq!(filter.playing.filter_type, FilterType::BandPass);
    out.extend(run(&mut filter, 24_001 + fade_frames(), 24_000));
    let steady = [
        FilterType::LowPass,
        FilterType::HighPass,
        FilterType::BandPass,
    ]
    .map(|t| {
        let mut alone = Filter::new(RATE).with_filter_type(t).with_cutoff(1000.0);
        largest_step(&run(&mut alone, 0, 24_000)[12_000..])
    })
    .into_iter()
    .fold(0.0, f32::max);
    let mut joined = vec![before[before.len() - 1]];
    joined.extend_from_slice(&out[..3 * fade_frames()]);
    assert!(largest_step(&joined) <= 1.5 * steady);
}

#[test]
fn a_switch_before_the_filter_is_heard_lands_at_once() {
    let mut switched = Filter::new(RATE).with_cutoff(1000.0);
    switched.set_filter_type(FilterType::HighPass);
    let mut built = Filter::new(RATE)
        .with_filter_type(FilterType::HighPass)
        .with_cutoff(1000.0);
    assert_eq!(run(&mut switched, 0, 2_000), run(&mut built, 0, 2_000));
}

#[test]
fn steep_and_resonant_switches_crossfade_without_a_step() {
    use FilterType::*;
    // (from, to, slope, resonance, slope toggled mid-fade)
    let cases = [
        (Bell, LowPass, 1.0, 0.3, false),
        (LowPass, HighPass, 1.0, 0.3, true),
        (HighPass, LowShelf, 0.0, 0.3, true),
        (LowPass, Bell, 0.0, 0.95, false),
    ];
    for (from, to, slope, resonance, toggle) in cases {
        let build = |filter_type| {
            let mut filter = Filter::new(RATE)
                .with_filter_type(filter_type)
                .with_cutoff(1000.0)
                .with_resonance(resonance);
            filter.set_control("slope", slope).unwrap();
            filter.set_control("boost", 12.0).unwrap();
            filter
        };
        let mut filter = build(from);
        let before = run(&mut filter, 0, 24_000);
        filter.set_filter_type(to);
        let mut across = run(&mut filter, 24_000, fade_frames() / 2);
        if toggle {
            filter.set_control("slope", 1.0 - slope).unwrap();
        }
        let rest = 24_000 + fade_frames() / 2;
        across.extend(run(&mut filter, rest, 2 * fade_frames()));
        let steady = [from, to]
            .map(|t| largest_step(&run(&mut build(t), 0, 24_000)[12_000..]))
            .into_iter()
            .fold(largest_step(&before[12_000..]), f32::max);
        let mut joined = vec![before[before.len() - 1]];
        joined.extend(across);
        let step = largest_step(&joined);
        assert!(
            step <= 1.5 * steady,
            "{from:?} -> {to:?}: {step} vs {steady}"
        );
    }
}
