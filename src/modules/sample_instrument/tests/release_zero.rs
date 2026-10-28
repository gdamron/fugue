//! A release of 0 or less stops a note at once without a click: it is
//! held at the steal fade's length, from config, a live write or a
//! rebuild, and reads back so. A note off under it fades over that many
//! frames, never fewer.

use super::{build_instrument, run, write_level_wav, TOL};
use crate::modules::sample_instrument::voice::DECLICK_SECONDS;
use crate::ControlValue;

const RATE: f32 = 44_100.0;

/// The steepest frame-to-frame drop a fade of `level` over the steal
/// fade's length makes (a little slack for 16-bit quantization).
fn steepest(level: f32) -> f32 {
    level / (DECLICK_SECONDS * RATE) + TOL
}

/// Releases a held note at 0.4 and checks it ends over the steal fade:
/// never dropping faster than it, and silent just after it.
fn ends_over_the_steal_fade(instrument: &mut Box<dyn crate::Module + Send>) {
    instrument.set_control("note_on", 60.0).unwrap();
    run(instrument, 2);
    let mut last = instrument.get_output("audio_left").unwrap();
    assert!((last - 0.4).abs() < TOL, "held at {last}");
    instrument.set_control("note_off", 60.0).unwrap();
    let frames = (DECLICK_SECONDS * RATE).ceil() as usize + 2;
    for frame in 0..frames {
        run(instrument, 1);
        let level = instrument.get_output("audio_left").unwrap();
        assert!(
            last - level <= steepest(0.4),
            "stepped {last} -> {level} at frame {frame}"
        );
        last = level;
    }
    assert_eq!(last, 0.0, "still sounding past the steal fade");
}

#[test]
fn a_release_of_zero_or_less_in_config_stops_a_note_at_once_without_a_click() {
    for release in [0.0, -1.0] {
        let path = write_level_wav(0.4, 44_100);
        let config = serde_json::json!({
            "release": release,
            "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
        });
        let (mut instrument, surface) = build_instrument(config);
        let held = surface.get_control("release").unwrap();
        assert_eq!(held, ControlValue::Number(DECLICK_SECONDS), "{release}");
        ends_over_the_steal_fade(&mut instrument);
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_release_of_zero_or_less_written_live_stops_a_note_at_once_without_a_click() {
    for release in [0.0, -5.0] {
        let path = write_level_wav(0.4, 44_100);
        let config = serde_json::json!({
            "release": 5.0,
            "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
        });
        let (mut instrument, _surface) = build_instrument(config);
        instrument.set_control("release", release).unwrap();
        assert_eq!(instrument.get_control("release").unwrap(), DECLICK_SECONDS);
        ends_over_the_steal_fade(&mut instrument);
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_release_that_is_not_a_number_is_still_refused() {
    let path = write_level_wav(0.4, 8);
    let config = serde_json::json!({
        "release": 0.2,
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, surface) = build_instrument(config);
    assert!(surface
        .set_control("release", ControlValue::Number(f32::NAN))
        .is_err());
    assert!(instrument.set_control("release", f32::INFINITY).is_err());
    assert_eq!(instrument.get_control("release").unwrap(), 0.2);
    let _ = std::fs::remove_file(path);
}

/// Automation (a schedule, a ramp) hands the instrument a release of 0 or
/// less as written, so it too holds the steal fade, not the range's floor:
/// the release declares no clamp of its own for that reason.
#[test]
fn a_release_of_zero_written_by_automation_holds_the_steal_fade() {
    let path = write_level_wav(0.4, 44_100);
    let config = serde_json::json!({
        "release": 5.0,
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, surface) = build_instrument(config);
    let automation = surface.automation("release").unwrap();
    assert!(automation.write_number(0.0));
    // As the audio thread takes it, just before the module processes.
    crate::control_request::take_automation(&mut *instrument);
    assert_eq!(instrument.get_control("release").unwrap(), DECLICK_SECONDS);
    ends_over_the_steal_fade(&mut instrument);
    let _ = std::fs::remove_file(path);
}
