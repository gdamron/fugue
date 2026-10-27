//! A note off and a release written for one sample: the note fades over
//! the new release, whichever was written first. A release written for a
//! later sample leaves the note off's fade as it began. (Writes at
//! different samples inside one block are split there by the audio
//! thread; the publish tests cover that path.)

use super::{build_instrument, run, write_level_wav, TOL};

/// A held note at 0.4, on an instrument whose release (1 ms) has run out
/// within 64 frames.
fn held_note() -> (Box<dyn crate::Module + Send>, std::path::PathBuf) {
    let path = write_level_wav(0.4, 44_100);
    let config = serde_json::json!({
        "release": 0.001,
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, _surface) = build_instrument(config);
    instrument.set_control("note_on", 60.0).unwrap();
    run(&mut instrument, 1);
    (instrument, path)
}

fn heard(instrument: &dyn crate::Module) -> f32 {
    instrument.get_output("audio_left").unwrap()
}

#[test]
fn a_release_written_at_a_note_offs_sample_applies_to_it_in_either_order() {
    for release_first in [false, true] {
        let (mut instrument, path) = held_note();
        if release_first {
            instrument.set_control("release", 1.0).unwrap();
        }
        instrument.set_control("note_off", 60.0).unwrap();
        if !release_first {
            instrument.set_control("release", 1.0).unwrap();
        }
        run(&mut instrument, 64);
        // 64 frames into a one-second fade, the note is all but whole.
        let level = heard(instrument.as_ref());
        assert!(
            (level - 0.4).abs() < 0.01,
            "release first: {release_first}, heard {level}"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_release_written_a_sample_after_a_note_off_does_not_reach_back() {
    let (mut instrument, path) = held_note();
    instrument.set_control("note_off", 60.0).unwrap();
    run(&mut instrument, 1);
    let fading = heard(instrument.as_ref());
    assert!(
        fading > TOL && fading < 0.4,
        "the old fade has begun: {fading}"
    );
    instrument.set_control("release", 1.0).unwrap();
    run(&mut instrument, 64);
    assert!(
        heard(instrument.as_ref()).abs() < TOL,
        "the old release ended it"
    );
    let _ = std::fs::remove_file(path);
}
