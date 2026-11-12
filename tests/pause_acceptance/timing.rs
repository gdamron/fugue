//! Clocks and timed writes across a pause.

use crate::*;
use fugue::rpc::ControlWhen;

/// A clock's `elapsed` and `position` hold while paused (even for seconds)
/// and continue from the held beat: one beat after resume, give or take a
/// block of 0.01 beat.
#[test]
fn clocks_hold_while_paused_and_continue_from_the_beat() {
    for mode in MODES {
        let mut rig = Rig::start(mode, &document(TONE_HZ, true));
        rig.render(150 * BLOCK);
        rig.pause();
        rig.render(HOLD_BY);
        let (elapsed, position) = (rig.get("clock", "elapsed"), rig.get("clock", "position"));
        assert!(elapsed > 1.5, "{mode:?}: the clock ran ({elapsed})");
        for _ in 0..3 {
            rig.render(1000 * BLOCK);
            assert_eq!(rig.get("clock", "elapsed"), elapsed, "{mode:?}");
            assert_eq!(rig.get("clock", "position"), position, "{mode:?}");
        }
        rig.resume();
        rig.render(BEAT);
        let moved = rig.get("clock", "elapsed") - elapsed;
        assert!((moved - 1.0).abs() < 0.025, "{mode:?}: moved {moved} beats");
        let moved = rig.get("clock", "position") - position;
        assert!(
            (moved - 1.0).abs() < 0.025,
            "{mode:?}: position moved {moved} beats"
        );
    }
}

/// A write timed on a beat, a bar or a sample, made before the pause and not
/// yet due, waits through seven beats of pause and lands on the same beat
/// (counted in the clock's beats) as with no pause. The clock is at 1.5
/// beats when each is written and 1.7 when the pause begins.
#[test]
fn timed_writes_wait_through_a_pause_and_land_on_their_beat() {
    let bar = ControlWhen::NextBar {
        beats_per_bar: Some(4.0),
        clock: None,
    };
    let every_three = ControlWhen::NextBeat {
        every: 3.0,
        offset: 0.0,
        clock: None,
    };
    let after_beats = ControlWhen::AfterBeats {
        beats: 2.0,
        clock: None,
    };
    let cases: Vec<(&str, ControlWhen, f32)> = vec![
        ("next bar", bar, 4.0),
        ("next third beat", every_three, 3.0),
        ("after two beats", after_beats, 3.5),
        (
            "at sample 19200",
            ControlWhen::AtSample { sample: 19_200 },
            3.0,
        ),
        (
            "after 32000 samples",
            ControlWhen::AfterSamples { samples: 32_000 },
            6.5,
        ),
    ];
    for (name, when, lands_on) in cases {
        for pause in [false, true] {
            let mut rig = Rig::start(Mode::Live, &document(TONE_HZ, true));
            rig.render(150 * BLOCK);
            let written = rig.get("clock", "elapsed");
            assert!(
                (written - 1.5).abs() < 0.001,
                "{name}: written at {written}"
            );
            rig.timed("osc", "frequency", 375.0, when.clone());
            rig.render(20 * BLOCK);
            if pause {
                rig.pause();
                rig.render(700 * BLOCK);
                assert_eq!(
                    rig.get("osc", "frequency"),
                    187.5,
                    "{name}: waits while held"
                );
                rig.resume();
            }
            let mut landed = None;
            for _ in 0..1000 {
                rig.render(BLOCK);
                if rig.get("osc", "frequency") == 375.0 {
                    landed = Some(rig.get("clock", "elapsed"));
                    break;
                }
            }
            let landed = landed.unwrap_or_else(|| panic!("{name}: never landed (pause {pause})"));
            // Seen at the end of its block: at most a block (0.01 beat) late.
            let late = landed - lands_on;
            assert!(
                (-0.0005..=0.0205).contains(&late),
                "{name}: landed {late} beats off (pause {pause})"
            );
        }
    }
}
