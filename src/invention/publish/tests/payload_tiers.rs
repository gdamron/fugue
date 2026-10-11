//! Payload writes rank and hold as values do (A4): at one sample the
//! higher tier wins whatever the order, and a late write is weighed at its
//! due sample. A payload that loses is retired, never freed on the audio
//! thread, and every block stays allocation- and free-free.

use super::payload_controls::{output, rig};
use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::{Outcome, Request, RequestId, RequestValue, Source, When};
use crate::payload::Payload;
use crate::test_support::tape::{freed, Notes, NOTES};

/// Submits notes `[value]` for the tape at sample `at`, from `source`.
fn submit_notes(rig: &Rig, value: f32, at: u64, source: Source) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("tape", NOTES).unwrap();
    let notes = Payload::new(Notes(vec![value]));
    let mut request = Request::new(target, RequestValue::Payload(notes));
    request.when = When::AtSample(at);
    request.source = source;
    rig.live.requests.submit(request).unwrap()
}

/// One block, counted: no allocation and no free, and no payload freed on
/// this (the audio) thread.
fn block(rig: &mut Rig) {
    let before = freed();
    assert_eq!(counted_block(rig), (0, 0));
    assert_eq!(freed(), before, "retired, not freed, on the audio thread");
}

#[test]
fn at_one_sample_the_higher_tier_wins_a_payload_and_the_loser_is_retired() {
    let mut rig = rig();
    rig.live.reclaim();
    let before = freed();
    let at = rig.graph.current_sample + 5;
    let script = submit_notes(&rig, 0.5, at, Source::Script);
    let user = submit_notes(&rig, 2.0, at, Source::User);
    let late = submit_notes(&rig, 0.125, at, Source::Script);
    block(&mut rig);
    assert_eq!(output(&rig), 2.0);
    assert_eq!(
        outcomes(&mut rig),
        [
            (script, Outcome::Overridden),
            (late, Outcome::Overridden),
            (user, Outcome::Applied { at }),
        ]
    );
    rig.live.reclaim();
    assert_eq!(freed(), before + 3, "both losers and the notes replaced");
}

#[test]
fn a_late_payload_losing_to_a_held_one_is_retired() {
    let mut rig = rig();
    let at = rig.graph.current_sample + 5;
    let user = submit_notes(&rig, 2.0, at, Source::User);
    block(&mut rig);
    rig.live.reclaim();
    let before = freed();
    // A block late, due before the user's: it loses to it at apply.
    let late = submit_notes(&rig, 0.5, at - 2, Source::Script);
    block(&mut rig);
    assert_eq!(output(&rig), 2.0);
    assert_eq!(
        outcomes(&mut rig),
        [(user, Outcome::Applied { at }), (late, Outcome::Overridden)]
    );
    rig.live.reclaim();
    assert_eq!(freed(), before + 1, "the loser");
}

/// A payload written at once, as an offline render writes one, holds its
/// control as a user's request does.
#[test]
fn a_payload_written_at_once_holds_its_control() {
    let mut rig = rig();
    let at = rig.graph.current_sample;
    let notes = Payload::new(Notes(vec![2.0]));
    rig.graph.apply_payload("tape", NOTES, notes).unwrap();
    let script = submit_notes(&rig, 0.5, at, Source::Script);
    block(&mut rig);
    assert_eq!(output(&rig), 2.0);
    assert_eq!(outcomes(&mut rig), [(script, Outcome::Overridden)]);
}
