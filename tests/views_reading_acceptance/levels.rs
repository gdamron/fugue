//! `levels` equal what `take_levels` would have returned at that
//! point, and the next read starts over like `take_levels` does.

use super::support::*;
use fugue::RenderEngine;

#[test]
fn levels_equal_take_levels_on_an_identical_engine() {
    for decibels in [12.0, -9.0] {
        let mut views = engine(decibels);
        let mut plain = engine(decibels);
        render(&mut views, 9_600, 256);
        render(&mut plain, 9_600, 256);
        let reading = views.take_view_reading(Some(CLOCK)).unwrap();
        let expected = plain.take_levels().unwrap();
        assert_eq!(reading.levels, expected, "{decibels} dB");
        assert_eq!(reading.levels.frames, 9_600);
        if decibels > 0.0 {
            assert!(reading.levels.left_overs > 0, "the loud piece has overs");
        }
    }
}

#[test]
fn levels_do_not_depend_on_the_clock_argument() {
    for clock in [Some(CLOCK), Some("no_such_clock"), None] {
        let mut views = engine(12.0);
        let mut plain = engine(12.0);
        render(&mut views, 4_800, 256);
        render(&mut plain, 4_800, 256);
        assert_eq!(
            views.take_view_reading(clock).unwrap().levels,
            plain.take_levels().unwrap(),
            "{clock:?}"
        );
    }
}

#[test]
fn the_next_read_starts_over() {
    let mut views = engine(12.0);
    let mut plain = engine(12.0);
    render(&mut views, 9_600, 256);
    render(&mut plain, 9_600, 256);
    assert_eq!(
        views.take_view_reading(Some(CLOCK)).unwrap().levels,
        plain.take_levels().unwrap()
    );
    // A second stretch is measured alone, as take_levels measures it.
    render(&mut views, 3_200, 256);
    render(&mut plain, 3_200, 256);
    let second = views.take_view_reading(Some(CLOCK)).unwrap().levels;
    assert_eq!(second.frames, 3_200);
    assert_eq!(second, plain.take_levels().unwrap());
    // And an immediate read finds an empty stretch.
    let empty = views.take_view_reading(Some(CLOCK)).unwrap().levels;
    assert_eq!(empty.frames, 0);
    assert_eq!(empty, plain.take_levels().unwrap());
}

#[test]
fn a_view_read_resets_the_window_take_levels_reads() {
    // The two share one window: after a view read, take_levels sees only
    // what was rendered since.
    let mut engine = engine(12.0);
    render(&mut engine, 6_400, 256);
    engine.take_view_reading(Some(CLOCK)).unwrap();
    render(&mut engine, 1_280, 256);
    assert_eq!(engine.take_levels().unwrap().frames, 1_280);
}

#[test]
fn nothing_loaded_reads_none() {
    let mut engine = RenderEngine::new(RATE);
    assert!(engine.take_view_reading(Some(CLOCK)).is_none());
    assert!(engine.take_view_reading(None).is_none());
}
