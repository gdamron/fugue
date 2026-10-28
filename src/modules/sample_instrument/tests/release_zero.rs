//! A release of 0 or less stops a note at once without a click: it is
//! held at 0, from config, a live write or a rebuild, and reads back so. A
//! note off under it eases out along the declick a stolen voice fades by,
//! never faster, with no corner where it starts or ends.

use super::{build_instrument, run, write_level_wav, TOL};
use crate::dsp::DECLICK_SECONDS;
use crate::ControlValue;

const RATE: f32 = 44_100.0;

/// The steepest frame-to-frame drop the declick of `level` makes: 1.5
/// times a straight fade's, at its middle (a little slack for 16-bit
/// quantization).
fn steepest(level: f32) -> f32 {
    1.5 * level / (DECLICK_SECONDS * RATE) + TOL
}

/// Releases a held note at 0.4 and checks it ends along the declick:
/// never dropping faster than it, never bending its slope by a straight
/// fade's corner (0.003 here), and silent just after it.
fn ends_over_the_steal_fade(instrument: &mut Box<dyn crate::Module + Send>) {
    instrument.set_control("note_on", 60.0).unwrap();
    run(instrument, 2);
    let mut last = instrument.get_output("audio_left").unwrap();
    assert!((last - 0.4).abs() < TOL, "held at {last}");
    let mut drop = 0.0;
    instrument.set_control("note_off", 60.0).unwrap();
    let frames = (DECLICK_SECONDS * RATE).ceil() as usize + 2;
    for frame in 0..frames {
        run(instrument, 1);
        let level = instrument.get_output("audio_left").unwrap();
        assert!(
            last - level <= steepest(0.4),
            "stepped {last} -> {level} at frame {frame}"
        );
        let bend = (last - level - drop).abs();
        assert!(bend < 0.001, "bent by {bend} at frame {frame}");
        drop = last - level;
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
        assert_eq!(held, ControlValue::Number(0.0), "{release}");
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
        assert_eq!(instrument.get_control("release").unwrap(), 0.0);
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
/// less as written, so it too stops at once, not over the range's floor:
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
    assert_eq!(instrument.get_control("release").unwrap(), 0.0);
    ends_over_the_steal_fade(&mut instrument);
    let _ = std::fs::remove_file(path);
}

/// A rebuild hands the instrument the release it reads back. A stop at
/// once reads back 0, so it still eases out along the curve, while a
/// positive release of the declick's own length still fades straight down,
/// as it always has.
#[test]
fn a_rebuild_keeps_a_stop_at_once_apart_from_a_release_of_its_length() {
    let path = write_level_wav(0.4, 44_100);
    let config = |release: f32| {
        serde_json::json!({
            "release": release,
            "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
        })
    };
    for (written, curved) in [(0.0, true), (DECLICK_SECONDS, false)] {
        let (mut instrument, _surface) = build_instrument(config(5.0));
        instrument.set_control("release", written).unwrap();
        let held = instrument.get_control("release").unwrap();
        let (mut rebuilt, _surface) = build_instrument(config(held));
        rebuilt.set_control("note_on", 60.0).unwrap();
        run(&mut rebuilt, 2);
        let held_at = rebuilt.get_output("audio_left").unwrap();
        rebuilt.set_control("note_off", 60.0).unwrap();
        run(&mut rebuilt, 1);
        let first = rebuilt.get_output("audio_left").unwrap();
        run(&mut rebuilt, 1);
        let second = rebuilt.get_output("audio_left").unwrap();
        assert_eq!(first, held_at, "{written}: the fade starts at full");
        let drop = first - second;
        // A straight fade's first drop is its whole slope; the curve's is
        // about 3 steps squared of it.
        let straight = held_at / (DECLICK_SECONDS * RATE);
        if curved {
            assert!(drop < 0.05 * straight, "{written}: dropped {drop}");
        } else {
            assert!((drop - straight).abs() < 1e-5, "{written}: dropped {drop}");
        }
    }
    let _ = std::fs::remove_file(path);
}
