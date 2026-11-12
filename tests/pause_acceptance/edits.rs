//! Writes and edits made while paused, a reload, and an unload.

use crate::*;

/// Pauses a tone, lets the fade end, and renders a little more held.
fn paused(mode: Mode, doc: &Value) -> Rig {
    let mut rig = Rig::start(mode, doc);
    rig.render(BEFORE);
    rig.pause();
    rig.render(HOLD_BY);
    rig
}

/// Resumes and returns the 79 ms that follow, past the fade in.
fn heard_after_resume(rig: &mut Rig) -> Vec<f32> {
    rig.resume();
    rig.render(4800)[1000..].to_vec()
}

/// A control write made while paused applies while held (the output stays
/// exactly 0.0) and is heard after resume: 375 Hz crosses zero twice per
/// cycle, about 59 times in the window, against 30 at 187.5 Hz.
#[test]
fn a_control_write_made_while_paused_is_heard_after_resume() {
    for mode in MODES {
        let mut rig = paused(mode, &document(TONE_HZ, true));
        rig.set("osc", "frequency", 375.0);
        assert!(all_zero(&rig.render(128)), "{mode:?}: held");
        assert_eq!(
            rig.get("osc", "frequency"),
            375.0,
            "{mode:?}: applied while held"
        );
        let heard = heard_after_resume(&mut rig);
        let count = crossings(&heard);
        assert!((55..=63).contains(&count), "{mode:?}: {count} crossings");
    }
}

/// A connect made while paused is silent until resume and then heard; a
/// disconnect made while paused is then silent.
#[test]
fn a_structural_edit_made_while_paused_is_heard_after_resume() {
    for mode in MODES {
        let mut rig = paused(mode, &document(TONE_HZ, false));
        rig.connect("osc", "dac");
        assert!(all_zero(&rig.render(128)), "{mode:?}: held");
        let heard = heard_after_resume(&mut rig);
        assert!(max_abs(&heard) > 0.1, "{mode:?}: connected tone heard");

        let mut rig = paused(mode, &document(TONE_HZ, true));
        rig.disconnect("osc", "dac");
        assert!(all_zero(&rig.render(128)), "{mode:?}: held");
        let heard = heard_after_resume(&mut rig);
        assert!(
            max_abs(&heard[2000..]) < 1e-4,
            "{mode:?}: disconnected tone silent"
        );
    }
}

/// A live reload made while paused succeeds, leaves the piece paused, and
/// its change is heard after resume.
#[test]
fn a_reload_while_paused_succeeds_and_stays_paused() {
    let mut rig = paused(Mode::Live, &document(TONE_HZ, true));
    rig.reload(&document(375.0, true));
    assert!(rig.is_paused());
    assert!(all_zero(&rig.render(128)), "still held after the reload");
    let status = rig.status();
    assert!(status.running && status.paused && !status.playing());
    let heard = heard_after_resume(&mut rig);
    let count = crossings(&heard);
    assert!((55..=63).contains(&count), "{count} crossings");
}

/// Unloading while paused is `RunningInvention::stop`, as while playing; a
/// piece loaded afterwards plays without a resume.
#[test]
fn unloading_while_paused_stops_and_a_new_load_plays() {
    paused(Mode::Live, &document(TONE_HZ, true)).unload();
    let mut next = Rig::start(Mode::Live, &document(TONE_HZ, true));
    assert!(!next.is_paused());
    assert!(max_abs(&next.render(BEFORE)) > 0.1);
}

/// Offline, a pause before any load does nothing, and a load after a pause
/// plays without a resume.
#[test]
fn an_offline_load_plays_without_a_resume() {
    let engine = fugue::RenderEngine::new(RATE);
    engine.pause();
    assert!(!engine.is_paused(), "nothing to pause before a load");
    let mut rig = paused(Mode::Offline, &document(TONE_HZ, true));
    assert!(rig.is_paused());
    let mut fresh = Rig::start(Mode::Offline, &document(TONE_HZ, true));
    assert!(!fresh.is_paused());
    assert!(max_abs(&fresh.render(BEFORE)) > 0.1);
    assert!(all_zero(&rig.render(128)));
}
