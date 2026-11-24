//! `beats` equals the clock's `elapsed` control as `get_control`
//! reports it (within f32 precision) for an existing clock, and is None for
//! a missing clock or None.

use super::support::*;

#[test]
fn beats_equal_the_clocks_elapsed_control() {
    let mut engine = engine(-9.0);
    let mut last = 0.0f64;
    // Several reads at odd buffer sizes, the control read just before each.
    for (frames, chunk) in [(4_800, 256), (9_600, 100), (24_000, 777), (480, 64)] {
        render(&mut engine, frames, chunk);
        let control = elapsed(&engine);
        let beats = engine
            .take_view_reading(Some(CLOCK))
            .unwrap()
            .beats
            .expect("the clock exists");
        assert!(
            same_beats(beats, control),
            "beats {beats}, control {control}"
        );
        assert!(beats > last, "the clock moves on: {beats} after {last}");
        last = beats;
    }
    // 38_880 frames at 48 kHz is 0.81 s, 1.62 beats at 120 bpm.
    assert!((last - 1.62).abs() < 0.05, "{last}");
}

#[test]
fn a_missing_clock_or_none_has_no_beats() {
    let mut engine = engine(12.0);
    render(&mut engine, 9_600, 256);
    for clock in [Some("no_such_clock"), Some(""), Some("osc"), None] {
        let reading = engine.take_view_reading(clock).unwrap();
        assert_eq!(reading.beats, None, "{clock:?}");
        assert!(reading.levels.frames > 0 || clock != Some("no_such_clock"));
    }
}

#[test]
fn reading_views_leaves_the_clock_alone() {
    // A read is not a rewind: the clock reads the same just after it.
    let mut engine = engine(-9.0);
    render(&mut engine, 12_000, 256);
    let before = elapsed(&engine);
    let beats = engine
        .take_view_reading(Some(CLOCK))
        .unwrap()
        .beats
        .unwrap();
    assert!(same_beats(beats, before));
    assert!(same_beats(elapsed(&engine), before));
}
