use super::*;
use crate::control_request::{apply_declared, RtValue};
use crate::modules::clock::controls::{BPM, POSITION, RESET};
use crate::Module;

/// Tempi with fractional samples per beat at 48 kHz, 44.1 kHz and 22.05 kHz,
/// plus a whole one (120 bpm at 48 kHz) and a fast pulse clock.
const CASES: &[(u32, f64)] = &[
    (48_000, 120.0),
    (48_000, 97.0),
    (44_100, 133.3),
    (22_050, 71.0),
    (48_000, 22_500.0),
];

/// The samples the clock outputs until its position first reaches `beat`,
/// counting the next as 1, by running it.
fn ran_until(clock: &mut Clock, beat: f64) -> u64 {
    let mut samples = 0;
    loop {
        clock.tick();
        samples += 1;
        if clock.beat_position() >= beat {
            return samples;
        }
    }
}

fn set(clock: &mut Clock, control: crate::control_request::ControlIndex, value: RtValue) {
    apply_declared(clock, control, value).unwrap();
}

#[test]
fn a_new_clock_is_before_beat_0_and_its_first_sample_starts_it() {
    for &(rate, bpm) in CASES {
        let mut clock = Clock::new(rate, bpm);
        assert_eq!(clock.position(), BEFORE_START);
        assert_eq!(clock.samples_until(0.0), Some(1));
        clock.tick();
        assert_eq!(clock.position(), 1.0 / clock.samples_per_beat());
    }
}

/// A clock at `bpm` that has output `warmup` samples, then had its tempo
/// set to `new_bpm` (if any) without outputting another.
fn run(rate: u32, bpm: f64, warmup: u64, new_bpm: Option<f64>) -> Clock {
    let mut clock = Clock::new(rate, bpm);
    (0..warmup).for_each(|_| clock.tick());
    if let Some(new_bpm) = new_bpm {
        set(&mut clock, BPM, RtValue::F32(new_bpm as f32));
    }
    clock
}

/// Asserts the clock's predictions (positions and samples until beats)
/// agree bit for bit with what a twin of it then outputs.
fn assert_predictions_hold(rate: u32, bpm: f64, warmup: u64, new_bpm: Option<f64>) {
    let clock = run(rate, bpm, warmup, new_bpm);
    let context = format!("{rate} Hz, {bpm} bpm, {warmup} samples, then {new_bpm:?}");
    let mut twin = run(rate, bpm, warmup, new_bpm);
    for k in 1..=5 {
        twin.tick();
        assert_eq!(
            twin.position().to_bits(),
            clock.position_after(k).to_bits(),
            "{context}"
        );
    }
    let whole = clock.position().max(0.0).floor();
    for beat in [0.0, 0.5, 1.0, 3.0, 4.0, 7.25, 16.0] {
        let target = whole + beat;
        let mut twin = run(rate, bpm, warmup, new_bpm);
        assert_eq!(
            clock.samples_until(target),
            Some(ran_until(&mut twin, target)),
            "{context}: beat {target}"
        );
    }
}

#[test]
fn the_samples_until_a_beat_are_the_samples_the_clock_takes_to_reach_it() {
    for &(rate, bpm) in CASES {
        for warmup in [0u64, 1, 37, 1_000, 48_001] {
            assert_predictions_hold(rate, bpm, warmup, None);
        }
    }
}

#[test]
fn a_pending_tempo_change_is_predicted_from_the_next_sample() {
    for &(rate, bpm) in CASES {
        for warmup in [0u64, 1, 211, 30_000] {
            for factor in [0.5, 1.37, 3.0] {
                assert_predictions_hold(rate, bpm, warmup, Some(bpm * factor));
            }
        }
    }
}

#[test]
fn a_tempo_that_never_reaches_a_beat_has_no_samples_until_it() {
    let mut clock = Clock::new(48_000, 120.0);
    clock.tick();
    for bpm in [0.0f32, -60.0] {
        set(&mut clock, BPM, RtValue::F32(bpm));
        assert_eq!(clock.samples_until(4.0), None, "{bpm} bpm");
    }
}

/// Asserts `clock`, just reset, starts beat 0 at its next sample, then
/// runs on as a new clock at `bpm` does from its first.
fn assert_starts_over(mut clock: Clock, rate: u32, bpm: f64, context: &str) {
    assert_eq!(clock.position(), BEFORE_START, "{context}");
    assert_eq!(clock.samples_until(0.0), Some(1), "{context}");
    let mut twin = Clock::new(rate, bpm);
    let predicted = clock.samples_until(2.0);
    clock.tick();
    assert_eq!(clock.position(), 0.0, "{context}");
    for port in 0..5 {
        assert_eq!(
            clock.output_block(port)[0],
            1.0,
            "{context}: every gate starts"
        );
    }
    let (mut samples, mut reached) = (1, None);
    for _ in 0..(3.0 * clock.samples_per_beat()) as usize {
        clock.tick();
        twin.tick();
        samples += 1;
        assert_eq!(
            clock.position().to_bits(),
            twin.position().to_bits(),
            "{context}"
        );
        for port in 0..5 {
            assert_eq!(clock.output_block(port)[0], twin.output_block(port)[0]);
        }
        if clock.position() >= 2.0 && reached.is_none() {
            reached = Some(samples);
        }
    }
    assert_eq!(predicted, reached, "{context}");
}

#[test]
fn a_reset_starts_beat_0_at_the_next_sample() {
    for &(rate, bpm) in CASES {
        for warmup in [0u64, 1, 100, 4_567] {
            let mut clock = Clock::new(rate, bpm);
            (0..warmup).for_each(|_| clock.tick());
            let begun = (clock.position().floor() + 1.0) as u64;
            set(&mut clock, RESET, RtValue::Bool(true));
            assert_eq!(clock.beats_before(), begun);
            // A second reset before any sample begins no beat.
            set(&mut clock, RESET, RtValue::Bool(true));
            assert_eq!(clock.beats_before(), begun);
            // The event leaves the control false, ready to fire again.
            assert_eq!(clock.cells.load(RESET), Some(RtValue::Bool(false)));
            assert_starts_over(clock, rate, bpm, &format!("{rate} Hz {bpm} bpm {warmup}"));
        }
    }
}

#[test]
fn a_reset_with_a_tempo_change_at_the_same_sample_starts_over_at_the_new_tempo() {
    for reset_first in [true, false] {
        let mut clock = Clock::new(44_100, 97.0);
        (0..1_234).for_each(|_| clock.tick());
        let writes = [(RESET, RtValue::Bool(true)), (BPM, RtValue::F32(151.0))];
        let order = if reset_first { [0, 1] } else { [1, 0] };
        for i in order {
            set(&mut clock, writes[i].0, writes[i].1);
        }
        assert_starts_over(clock, 44_100, 151.0, &format!("reset first: {reset_first}"));
    }
}

#[test]
fn a_false_reset_changes_nothing() {
    let mut clock = Clock::new(48_000, 120.0);
    (0..999).for_each(|_| clock.tick());
    let held = clock.position();
    set(&mut clock, RESET, RtValue::Bool(false));
    assert_eq!((clock.position(), clock.beats_before()), (held, 0));
}

#[test]
fn processing_publishes_the_position() {
    let mut clock = Clock::new(48_000, 22_500.0);
    assert_eq!(clock.cells.load(POSITION), Some(RtValue::F32(0.0)));
    clock.process(64);
    assert_eq!(clock.cells.load(POSITION), Some(RtValue::F32(0.5)));
    assert!(clock.set_control("position", 3.0).is_err(), "read-only");
}

#[test]
fn reset_on_reload_is_off_unless_the_config_sets_it() {
    use serde_json::json;
    assert!(writes_on_reload(&json!({})).is_empty());
    assert!(writes_on_reload(&json!({ "reset_on_reload": false })).is_empty());
    assert_eq!(
        writes_on_reload(&json!({ "bpm": 90, "reset_on_reload": true })),
        [("reset", ControlValue::Bool(true))]
    );
    assert!(reset_on_reload(&json!({ "reset_on_reload": 1 })).is_err());
}
