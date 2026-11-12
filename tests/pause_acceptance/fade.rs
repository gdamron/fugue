//! The fade out, the hold, the fade in, idempotence, and the status.

use crate::*;

/// A pause then a resume of a sustained tone, from four phases: the output
/// fades out within about 10 ms (not cut at once), holds at exactly 0.0, and
/// fades back in (not a jump); no step across it all exceeds the tone's own
/// largest step plus the fade's A / 480.
#[test]
fn pause_and_resume_fade_without_a_click() {
    for mode in MODES {
        for lead in [BEFORE, BEFORE + 64, BEFORE + 128, BEFORE + 192] {
            let label = format!("{mode:?}, {lead} frames of lead");
            let mut rig = Rig::start(mode, &document(TONE_HZ, true));
            let mut y = rig.render(lead);
            let a = max_abs(&y[lead - 512..]);
            assert!(a > 0.1, "{label}: the tone sounds ({a})");

            rig.pause();
            y.extend(rig.render(HOLD_BY));
            // A fade, not a cut: a whole cycle starting 128 frames in is
            // still well above silence (gain at least 0.2 there).
            let fading = max_abs(&y[lead + 128..lead + 384]);
            assert!(
                fading > 0.1 * a,
                "{label}: still sounding at 128..384 ({fading})"
            );
            y.extend(rig.render(2048));
            assert!(
                all_zero(&y[lead + HOLD_BY..]),
                "{label}: held output is exactly 0.0"
            );

            rig.resume();
            let resumed = y.len();
            y.extend(rig.render(2048));
            let first = max_abs(&y[resumed..resumed + 64]);
            assert!(first < 0.25 * a, "{label}: rises from silence ({first})");
            let full = max_abs(&y[resumed + HOLD_BY..]);
            assert!(
                full >= 0.99 * a,
                "{label}: back at full level ({full} of {a})"
            );

            let tone_step = a * (std::f32::consts::TAU * TONE_HZ as f32 / RATE as f32);
            let limit = tone_step + a / 480.0 + 1e-5;
            let step = max_step(&y, 1024);
            assert!(step <= limit, "{label}: step {step} exceeds {limit}");
        }
    }
}

/// A second pause does not restart the fade; a resume on a playing piece
/// changes nothing: both leave the output sample for sample as without.
#[test]
fn pausing_twice_and_resuming_a_playing_piece_change_nothing() {
    for mode in MODES {
        let tail = 2048;
        let mut once = Rig::start(mode, &document(TONE_HZ, true));
        once.render(BEFORE);
        once.pause();
        let expected = once.render(tail);

        let mut twice = Rig::start(mode, &document(TONE_HZ, true));
        twice.render(BEFORE);
        twice.pause();
        let mut got = twice.render(192);
        twice.pause();
        got.extend(twice.render(tail - 192));
        assert!(
            got == expected,
            "{mode:?}: a second pause changed the output"
        );
        assert!(twice.is_paused());

        let mut plain = Rig::start(mode, &document(TONE_HZ, true));
        let expected = plain.render(BEFORE + tail);
        let mut resumed = Rig::start(mode, &document(TONE_HZ, true));
        let mut got = resumed.render(BEFORE);
        resumed.resume();
        assert!(!resumed.is_paused());
        got.extend(resumed.render(tail));
        assert!(
            got == expected,
            "{mode:?}: resuming a playing piece changed the output"
        );
    }
}

/// While paused `paused` is true and `playing()` false, with `running` still
/// true; after resume the reverse. `paused` is on the wire only when true.
#[test]
fn status_reports_paused_and_playing() {
    let paused_key = |rig: &Rig| {
        serde_json::to_value(rig.status())
            .unwrap()
            .get("paused")
            .cloned()
    };
    for mode in MODES {
        let mut rig = Rig::start(mode, &document(TONE_HZ, true));
        rig.render(BLOCK);
        let status = rig.status();
        assert!(
            status.running && !status.paused && status.playing(),
            "{mode:?}"
        );
        assert_eq!(paused_key(&rig), None, "omitted when false");

        rig.pause();
        assert!(rig.is_paused());
        rig.render(BLOCK);
        let status = rig.status();
        assert!(
            status.running && status.paused && !status.playing(),
            "{mode:?}"
        );
        assert_eq!(paused_key(&rig), Some(json!(true)));

        rig.resume();
        assert!(!rig.is_paused());
        rig.render(BLOCK);
        let status = rig.status();
        assert!(
            status.running && !status.paused && status.playing(),
            "{mode:?}"
        );
        assert_eq!(paused_key(&rig), None);
    }
}
