//! A sample instrument's note off and a release on a live graph: written
//! for one sample, the note fades over the new release whichever was
//! sent first; written for different samples, in one block or across
//! two, or late together (an edit installing between them too), the note
//! off fades over the release in force at its own.

use super::requests::outcomes;
use super::sample_instrument::{rig, set, TOL};
use super::*;
use crate::control_request::{ControlIndex, Outcome, Request, RequestValue, RtValue, When};

/// The instrument's controls, in its listed order.
const RELEASE: ControlIndex = ControlIndex(0);
const NOTE_OFF: ControlIndex = ControlIndex(2);

/// Sends `value` to the instrument for `at`, as a front door would: a
/// note off as the event it is declared.
fn send(rig: &Rig, control: ControlIndex, value: f32, at: u64) {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("inst", control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = When::AtSample(at);
    request.event = control == NOTE_OFF;
    rig.live.requests.submit(request).unwrap();
}

/// A rig whose instrument plays into the DAC, its note 60 sounding at
/// 0.5 under a 1 ms release, and the sample its next block starts at.
fn sounding() -> (tempfile::TempDir, Rig, u64) {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    let wire =
        |change: &mut GraphChange| change.connect(edge("inst", "audio_left", "dac", "audio"));
    rig.live.edit(wire).unwrap();
    set(&rig, "note_on", 60.0.into()).unwrap();
    assert!((last(&rig.render(1)) - 0.5).abs() < TOL);
    outcomes(&mut rig);
    let start = rig.graph.current_sample;
    (dir, rig, start)
}

/// The last frame heard.
fn last(heard: &[f32]) -> f32 {
    heard[heard.len() - 1]
}

fn all_applied(rig: &mut Rig) {
    for (_, outcome) in outcomes(rig) {
        assert!(matches!(outcome, Outcome::Applied { .. }), "{outcome:?}");
    }
}

#[test]
fn a_release_sent_for_a_note_offs_sample_applies_to_it_in_either_order() {
    for release_first in [false, true] {
        let (_dir, mut rig, start) = sounding();
        let at = start + 10;
        if release_first {
            send(&rig, RELEASE, 1.0, at);
        }
        send(&rig, NOTE_OFF, 60.0, at);
        if !release_first {
            send(&rig, RELEASE, 1.0, at);
        }
        // 53 frames into a one-second fade, the note is all but whole.
        let level = last(&rig.render(1));
        assert!(
            (level - 0.5).abs() < 0.01,
            "release first: {release_first}, heard {level}"
        );
        all_applied(&mut rig);
    }
}

#[test]
fn a_release_sent_later_in_the_note_offs_block_does_not_reach_back() {
    let (_dir, mut rig, start) = sounding();
    send(&rig, NOTE_OFF, 60.0, start + 4);
    send(&rig, RELEASE, 1.0, start + 8);
    let heard = rig.render(1);
    assert!(heard[8] > TOL && heard[8] < 0.5, "fading: {}", heard[8]);
    assert!(last(&heard).abs() < TOL, "the old release ended it");
    all_applied(&mut rig);
}

#[test]
fn a_release_sent_for_the_next_block_does_not_reach_back() {
    let (_dir, mut rig, start) = sounding();
    send(&rig, NOTE_OFF, 60.0, start + 60);
    send(&rig, RELEASE, 1.0, start + 64);
    let heard = rig.render(2);
    assert!(heard[64] > TOL && heard[64] < 0.5, "fading: {}", heard[64]);
    assert!(last(&heard).abs() < TOL, "the old release ended it");
    all_applied(&mut rig);
}

#[test]
fn a_late_release_due_after_a_late_note_off_does_not_reach_back() {
    let (_dir, mut rig, start) = sounding();
    // Both due before the block they reach.
    send(&rig, NOTE_OFF, 60.0, start - 4);
    send(&rig, RELEASE, 1.0, start - 2);
    let heard = rig.render(1);
    assert!(last(&heard).abs() < TOL, "the old release ended it");
    let late: Vec<_> = (outcomes(&mut rig).into_iter())
        .map(|(_, outcome)| outcome)
        .collect();
    let applied = |due| Outcome::AppliedLate { at: start, due };
    assert_eq!(late, [applied(start - 4), applied(start - 2)]);
}

/// An edit installs between a late note off and a late release due after
/// it: the note off, queued before the edit, applies on the graph the edit
/// replaces; the release, queued after it, on the graph it installs. The
/// release does not reach back to the note off.
#[test]
fn a_late_release_queued_behind_an_edit_does_not_reach_back() {
    let (_dir, mut rig, start) = sounding();
    send(&rig, NOTE_OFF, 60.0, start - 4);
    let osc = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc));
    send(&rig, RELEASE, 1.0, start - 2);
    let heard = rig.render(1);
    assert!(
        last(&heard).abs() < TOL,
        "the old release ended it: {}",
        last(&heard)
    );
}
