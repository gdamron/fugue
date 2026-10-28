//! A sample instrument's note off and a release either side of an
//! immediate edit keeping it, at one block start: the note off fades over
//! the release in force at its due sample, as with no edit between them.
//! Each sequence is heard at the dac against a twin without the edit.

use super::requests::outcomes;
use super::sample_instrument::{rig, set, TOL};
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Outcome, Request, RequestValue, RtValue, When,
};

/// The instrument's controls, in its listed order.
const RELEASE: ControlIndex = ControlIndex(0);
const NOTE_OFF: ControlIndex = ControlIndex(2);

/// How a step is timed, from the block start everything is submitted at.
#[derive(Clone, Copy, Debug)]
enum Due {
    /// So many samples before it.
    Late(u64),
    Now,
    /// On the clock's beat at this block start.
    Beat,
}

#[derive(Clone, Copy)]
enum Step {
    /// A one-second release.
    Release(Due),
    /// Note 60's note off.
    NoteOff(Due),
    /// An edit keeping the instrument; the twin leaves it out.
    Edit(fn(&Rig)),
}

fn send(rig: &Rig, control: ControlIndex, value: f32, due: Due, start: u64) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("inst", control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = match due {
        Due::Late(by) => When::AtSample(start - by),
        Due::Now => When::Now,
        Due::Beat => When::Beat(BeatTime {
            clock: publisher
                .control_target("clock", ControlIndex(0))
                .unwrap()
                .module_idx as u32,
            spec: BeatSpec::After(0.0),
            ramp: None,
        }),
    };
    request.event = control == NOTE_OFF;
    rig.live.requests.submit(request).unwrap()
}

/// The instrument into the dac, note 60 sounding at 0.5 under a 1 ms
/// release, beside a clock at 120 bpm the dac does not hear.
fn sounding() -> (tempfile::TempDir, Rig) {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": 120.0 }));
    let wire = |change: &mut GraphChange| {
        change.upsert("clock", clock);
        change.connect(edge("inst", "audio_left", "dac", "audio"))
    };
    rig.live.edit(wire).unwrap();
    set(&rig, "note_on", 60.0.into()).unwrap();
    let heard = rig.render(1);
    assert!((heard[heard.len() - 1] - 0.5).abs() < TOL);
    outcomes(&mut rig);
    (dir, rig)
}

/// Plays `steps` from one block start (the edits only `with_edits`): the
/// output and each request's outcomes, in order.
fn play(steps: &[Step], with_edits: bool) -> (Vec<f32>, Vec<Vec<Outcome>>) {
    let (_dir, mut rig) = sounding();
    let start = rig.graph.current_sample;
    let mut ids = Vec::new();
    for step in steps {
        match *step {
            Step::Release(due) => ids.push(send(&rig, RELEASE, 1.0, due, start)),
            Step::NoteOff(due) => ids.push(send(&rig, NOTE_OFF, 60.0, due, start)),
            Step::Edit(edit) if with_edits => edit(&rig),
            Step::Edit(_) => {}
        }
    }
    let out = rig.render(4);
    let all = outcomes(&mut rig);
    let of = |id| {
        all.iter()
            .filter(|(got, _)| *got == id)
            .map(|(_, o)| *o)
            .collect()
    };
    (out, ids.into_iter().map(of).collect())
}

/// Asserts `steps` sound and settle as they would without their edits.
fn as_without_edits(steps: &[Step]) {
    let without = play(steps, false).1;
    assert_eq!(sounds_as_without_edits(steps), without);
}

/// Asserts `steps` sound as they would without their edits, returning
/// how they settle.
fn sounds_as_without_edits(steps: &[Step]) -> Vec<Vec<Outcome>> {
    let (out, settled) = play(steps, true);
    let (twin, _) = play(steps, false);
    let parted = out.iter().zip(&twin).position(|(a, b)| a != b);
    assert_eq!(parted, None, "parted at {parted:?}");
    settled
}

fn add_osc(rig: &Rig) {
    let osc = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc));
}

fn remove_clock(rig: &Rig) {
    rig.publish_unreclaimed(|change| change.remove("clock"));
}

/// A late release, then a note off due now, both before the edit: the
/// note fades over the release.
#[test]
fn a_note_off_due_now_before_an_edit_fades_over_the_late_release_before_it() {
    as_without_edits(&[
        Step::Release(Due::Late(2)),
        Step::NoteOff(Due::Now),
        Step::Edit(add_osc),
    ]);
}

/// A late note off before the edit, a release due after it behind: the
/// old release ends the note.
#[test]
fn a_late_release_behind_an_edit_does_not_reach_back_to_a_note_off() {
    as_without_edits(&[
        Step::NoteOff(Due::Late(4)),
        Step::Edit(add_osc),
        Step::Release(Due::Late(2)),
    ]);
}

/// A late release before the edit, a note off due before it behind: the
/// note off runs before the release staged ahead of it, so the old
/// release ends the note.
#[test]
fn a_late_note_off_behind_an_edit_runs_before_a_release_due_after_it() {
    as_without_edits(&[
        Step::Release(Due::Late(2)),
        Step::Edit(add_osc),
        Step::NoteOff(Due::Late(4)),
    ]);
}

/// A late release, then a note off on a beat of a clock the edit removes:
/// the note fades over the release.
#[test]
fn a_note_off_on_a_removed_clocks_beat_fades_over_the_late_release() {
    as_without_edits(&[
        Step::Release(Due::Late(2)),
        Step::NoteOff(Due::Beat),
        Step::Edit(remove_clock),
    ]);
}

/// A release on a beat of a clock the edit removes, then a note off due
/// now: the note fades over the release.
#[test]
fn a_note_off_due_now_fades_over_a_release_on_a_removed_clocks_beat() {
    as_without_edits(&[
        Step::Release(Due::Beat),
        Step::NoteOff(Due::Now),
        Step::Edit(remove_clock),
    ]);
}

/// Two releases before the edit, the later replacing the earlier, then a
/// note off due between them behind it: it fades over the earlier, in
/// force at its due sample (which reports Superseded, as it gave way).
#[test]
fn a_note_off_between_two_staged_releases_fades_over_the_earlier() {
    let settled = sounds_as_without_edits(&[
        Step::Release(Due::Late(6)),
        Step::Release(Due::Late(2)),
        Step::Edit(add_osc),
        Step::NoteOff(Due::Late(4)),
    ]);
    assert_eq!(settled[0], [Outcome::Superseded]);
}

/// A release before the edit, one due before it behind (losing to it),
/// then a note off due between them: it fades over the one behind.
#[test]
fn a_note_off_after_a_release_that_lost_behind_an_edit_fades_over_it() {
    let settled = sounds_as_without_edits(&[
        Step::Release(Due::Late(2)),
        Step::Edit(add_osc),
        Step::Release(Due::Late(6)),
        Step::NoteOff(Due::Late(4)),
    ]);
    assert_eq!(settled[1], [Outcome::Superseded]);
}

/// Two releases before the edit, then a note off due before both behind
/// it: the old release ends the note.
#[test]
fn a_note_off_before_two_staged_releases_reads_neither() {
    as_without_edits(&[
        Step::Release(Due::Late(3)),
        Step::Release(Due::Late(1)),
        Step::Edit(add_osc),
        Step::NoteOff(Due::Late(4)),
    ]);
}

/// A release replaced in an earlier block is gone once it flushed: a note
/// off behind an edit, due before a release staged ahead of it, reads the
/// release in force (the short one written last), not the replaced one.
#[test]
fn a_release_replaced_a_block_before_is_not_read_again() {
    let play = |with_edit: bool| {
        let (_dir, mut rig) = sounding();
        let start = rig.graph.current_sample;
        send(&rig, RELEASE, 1.0, Due::Late(6), start);
        send(&rig, RELEASE, 0.001, Due::Late(2), start);
        let mut out = rig.render(1);
        let start = rig.graph.current_sample;
        send(&rig, RELEASE, 1.0, Due::Late(2), start);
        if with_edit {
            add_osc(&rig);
        }
        send(&rig, NOTE_OFF, 60.0, Due::Late(4), start);
        out.extend(rig.render(4));
        out
    };
    assert_eq!(play(true), play(false));
}
