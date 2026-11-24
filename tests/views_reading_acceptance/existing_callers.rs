//! Existing callers are unchanged: `take_levels` and
//! `get_control` still answer as before.

use super::support::*;

#[test]
fn take_levels_answers_for_the_stretch_rendered() {
    let mut engine = engine(12.0);
    render(&mut engine, 9_600, 256);
    let levels = engine.take_levels().unwrap();
    assert_eq!(levels.frames, 9_600);
    assert!(levels.left_peak > 3.0 && levels.right_peak > 3.0);
    assert!(levels.left_overs > 0 && levels.right_overs > 0);
    assert_eq!(levels.non_finite_samples, 0);
    // It starts over, as it always did.
    render(&mut engine, 640, 64);
    assert_eq!(engine.take_levels().unwrap().frames, 640);
}

#[test]
fn get_control_elapsed_follows_the_tempo() {
    let mut engine = engine(-9.0);
    assert_eq!(elapsed(&engine), 0.0);
    // One second at 120 bpm is two beats.
    render(&mut engine, 48_000, 256);
    assert!(
        (elapsed(&engine) - 2.0).abs() < 0.02,
        "{}",
        elapsed(&engine)
    );
    render(&mut engine, 48_000, 256);
    assert!(
        (elapsed(&engine) - 4.0).abs() < 0.02,
        "{}",
        elapsed(&engine)
    );
}

#[test]
fn get_control_does_not_disturb_the_levels() {
    let mut engine = engine(12.0);
    render(&mut engine, 4_800, 256);
    elapsed(&engine);
    assert_eq!(engine.take_levels().unwrap().frames, 4_800);
}

#[test]
fn get_control_still_refuses_a_missing_module() {
    let engine = engine(-9.0);
    assert!(engine.get_control("no_such_clock", "elapsed").is_err());
}
