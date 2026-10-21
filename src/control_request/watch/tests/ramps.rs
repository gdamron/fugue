//! Ramps in beats on a scripted timeline: what a ramp writes for each
//! position of its clock, that it arrives exactly at its end, and that it
//! carries on through a reset.

use super::*;
use crate::control_request::ramps::{ramp_value, BeatRamp};

/// A clock at `position`, a beat every 128 samples.
fn at(position: f64, beats_before: u64) -> Scripted {
    Scripted {
        position,
        rate: 1.0 / 128.0,
        beats_before,
    }
}

/// A sample before `beat`: the clock's next sample is on it.
fn before(beat: f64) -> f64 {
    beat - 1.0 / 128.0
}

#[test]
fn a_ramp_glides_from_its_start_and_arrives_exactly_at_its_end() {
    let ramp = BeatRamp::new(0.25, 1.0, 1.0, 2.0, 0);
    assert_eq!(ramp.next(&at(before(1.0), 0)), Some((0.25, false)));
    let halfway = ramp_value(0.25, 1.0, 0.5);
    assert_eq!(ramp.next(&at(before(2.0), 0)), Some((halfway, false)));
    let short = ramp.next(&at(before(3.0) - 1.0 / 128.0, 0));
    assert!(matches!(short, Some((value, false)) if value < 1.0));
    assert_eq!(ramp.next(&at(before(3.0), 0)), Some((1.0, true)));
    assert_eq!(ramp.next(&at(5.0, 0)), Some((1.0, true)));
    // Its end sample is 256 after beat 1's, the one the clock plays next.
    assert_eq!(ramp.samples_to_end(&at(before(1.0), 0)), Some(257));
}

#[test]
fn a_ramp_carries_on_from_the_next_whole_beat_after_a_reset() {
    let ramp = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 0);
    // Reset at position 1.5: beat 2 is begun, so the count is 2 from the
    // clock's new beat 0, and the ramp has a beat to go.
    let reset = at(before(0.0), 2);
    assert_eq!(ramp.next(&reset), Some((0.5, false)));
    assert_eq!(ramp.samples_to_end(&reset), Some(129));
    assert_eq!(ramp.next(&at(before(1.0), 2)), Some((1.0, true)));
    // Counted from a clock that had begun beats already.
    let late = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 2);
    assert_eq!(late.next(&reset), Some((0.0, false)));
}

#[test]
fn a_ramp_on_a_clock_whose_position_is_no_number_writes_nothing() {
    let ramp = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 0);
    assert_eq!(ramp.next(&at(f64::NAN, 0)), None);
}

#[test]
fn ramp_values_stay_between_the_endpoints_and_finite() {
    assert_eq!(ramp_value(0.25, 1.0, -1.0), 0.25);
    assert_eq!(ramp_value(0.25, 1.0, 2.0), 1.0);
    assert_eq!(ramp_value(1.0, -1.0, 0.5), 0.0);
    assert_eq!(ramp_value(f32::MIN, f32::MAX, 0.5), 0.0);
    assert_eq!(ramp_value(f32::MIN, f32::MAX, 1.0), f32::MAX);
    assert_eq!(ramp_value(f32::MIN, f32::MAX, f64::NAN), f32::MIN);
    for step in 0..=64 {
        let value = ramp_value(f32::MIN, f32::MAX, f64::from(step) / 64.0);
        assert!(value.is_finite(), "{step}");
    }
}
