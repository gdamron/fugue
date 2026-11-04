//! The EQ responses, from inside the module.

use std::f32::consts::TAU;

use super::*;

const RATE: u32 = 48_000;

fn sine(n: usize, hz: f32) -> f32 {
    0.25 * (TAU * hz * n as f32 / RATE as f32).sin()
}

fn run(filter: &mut Filter, from: usize, frames: usize, hz: f32) -> Vec<f32> {
    (from..from + frames)
        .map(|n| {
            filter.set_input("audio", sine(n, hz)).unwrap();
            filter.process(1);
            filter.get_output("audio").unwrap()
        })
        .collect()
}

fn rms(signal: &[f32]) -> f32 {
    (signal.iter().map(|v| v * v).sum::<f32>() / signal.len() as f32).sqrt()
}

fn eq(filter_type: FilterType, cutoff: f32, boost: f32, q: f32) -> Filter {
    let mut filter = Filter::new(RATE)
        .with_filter_type(filter_type)
        .with_cutoff(cutoff);
    filter.set_control("boost", boost).unwrap();
    filter.set_control("q", q).unwrap();
    filter
}

#[test]
fn a_flat_eq_passes_its_input_bit_for_bit() {
    for filter_type in [
        FilterType::Bell,
        FilterType::LowShelf,
        FilterType::HighShelf,
    ] {
        let mut filter = eq(filter_type, 1000.0, 0.0, 4.0);
        let out = run(&mut filter, 0, 2_000, 997.0);
        let dry: Vec<f32> = (0..2_000).map(|n| sine(n, 997.0)).collect();
        assert_eq!(out, dry, "{filter_type:?}");
    }
}

#[test]
fn a_bell_at_20_khz_peaks_at_20_khz() {
    // `fast_tan` would put this cutoff near 19 kHz, where a narrow bell
    // misses 20 kHz by most of its boost.
    let mut filter = eq(FilterType::Bell, 20_000.0, 12.0, 8.0);
    let out = run(&mut filter, 0, 24_000, 20_000.0);
    let gain = 20.0 * (rms(&out[12_000..]) / (0.25 * std::f32::consts::FRAC_1_SQRT_2)).log10();
    assert!((gain - 12.0).abs() < 0.5, "{gain} dB");
}

#[test]
fn coefficients_are_designed_only_when_a_parameter_moves() {
    let mut design = EqDesign::new(RATE);
    design.design(FilterType::Bell, 1000.0, 6.0, 2.0);
    let first = design.coefficients;
    // A poisoned cache shows whether the next call recomputes.
    design.coefficients.k = f32::NAN;
    design.design(FilterType::Bell, 1000.0, 6.0, 2.0);
    assert!(design.coefficients.k.is_nan());
    design.design(FilterType::Bell, 1000.0, 6.0, 3.0);
    assert!(design.coefficients.k.is_finite());
    assert_ne!(design.coefficients.k, first.k);
}

#[test]
fn shelves_stop_steepening_past_q_2() {
    let mut at_two = EqDesign::new(RATE);
    at_two.design(FilterType::LowShelf, 500.0, 12.0, 2.0);
    let mut at_max = EqDesign::new(RATE);
    at_max.design(FilterType::LowShelf, 500.0, 12.0, 18.0);
    assert_eq!(at_two.coefficients.k, at_max.coefficients.k);
}

#[test]
fn a_notch_does_not_redesign_for_a_boost_it_ignores() {
    let mut design = EqDesign::new(RATE);
    design.design(FilterType::Notch, 1000.0, 0.0, 2.0);
    design.coefficients.k = f32::NAN;
    design.design(FilterType::Notch, 1000.0, 6.0, 2.0);
    assert!(design.coefficients.k.is_nan());
}
