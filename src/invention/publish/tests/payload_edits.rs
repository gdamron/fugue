//! Payload requests among structural edits and beats: one submitted before
//! an edit applies to the instance the edit replaces, one whose module an
//! edit removes or rebuilds is refused and freed by the reclaimer, and one
//! timed in beats applies on its beat. Every block is allocation- and
//! free-free (debug builds panic if a payload drops in one).

use super::request_beats::{beat_rig, outcomes_of, submit_level};
use super::requests::{counted_block, outcomes, try_submit_value};
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, Outcome, Refusal, Request, RequestId, RequestValue, When,
};
use crate::payload::Payload;
use crate::test_support::tape::{freed, Notes, TapeFactory, NOTES};

/// Adds a tape holding `[0.25]` into the dac.
fn add_tape(rig: &mut Rig) {
    rig.registry.register(TapeFactory);
    rig.adopt_registry();
    upsert_tape(rig, 0.25);
    rig.render(1);
}

/// Builds (or rebuilds) the tape, holding `[note]`.
fn upsert_tape(rig: &Rig, note: f32) {
    let tape = rig.build("tape", "tape", serde_json::json!({ "notes": [note] }));
    rig.live
        .edit(|change| {
            change.upsert("tape", tape);
            change.connect(edge("tape", "out", "dac", "audio"))
        })
        .unwrap();
}

fn notes(values: &[f32]) -> RequestValue {
    RequestValue::Payload(Payload::new(Notes(values.to_vec(), None)))
}

fn submit(rig: &Rig, values: &[f32], when: When) -> RequestId {
    try_submit_value(rig, "tape", NOTES.0, notes(values), when).unwrap()
}

fn output(rig: &Rig) -> f32 {
    rig.graph.modules["tape"].module().output_block(0)[0]
}

fn dac_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
        "version": "1.0.0",
        "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
        "connections": []
    }"#,
    );
    add_tape(&mut rig);
    rig.live.reclaim();
    rig
}

#[test]
fn a_payload_submitted_before_an_edit_applies_to_the_instance_it_replaces() {
    let mut rig = dac_rig();
    let start = rig.graph.current_sample;
    let before = freed();
    let first = submit(&rig, &[1.0], When::Now);
    upsert_tape(&rig, 0.5);
    let second = submit(&rig, &[2.0, 1.0], When::Now);

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(freed(), before, "nothing freed on the audio thread");
    let applied = Outcome::Applied { at: start };
    assert_eq!(outcomes(&mut rig), [(first, applied), (second, applied)]);
    assert_eq!(output(&rig), 3.0, "the second reached the rebuilt tape");

    // The old tape's notes, the first payload (kept by the old tape), and
    // the new tape's built notes, replaced by the second.
    rig.live.reclaim();
    assert_eq!(freed(), before + 3);
}

#[test]
fn a_waiting_payload_whose_module_an_edit_rebuilds_or_removes_is_refused_and_freed_off_thread() {
    for remove in [false, true] {
        let mut rig = dac_rig();
        let before = freed();
        let far = rig.graph.current_sample + 10_000;
        let waiting = submit(&rig, &[9.0], When::AtSample(far));
        if remove {
            rig.live
                .edit(|change| {
                    change.remove("tape");
                    Ok(())
                })
                .unwrap();
        } else {
            upsert_tape(&rig, 0.5);
        }

        assert_eq!(counted_block(&mut rig), (0, 0));
        assert_eq!(freed(), before);
        assert_eq!(
            outcomes(&mut rig),
            [(waiting, Outcome::Refused(Refusal::TargetGone))]
        );
        // The refused payload and the old tape's notes.
        rig.live.reclaim();
        assert_eq!(freed(), before + 2, "remove: {remove}");
    }
}

#[test]
fn a_payload_timed_in_beats_applies_on_its_beat() {
    let mut rig = beat_rig(480.0);
    add_tape(&mut rig);
    rig.live.reclaim();
    let start = rig.graph.current_sample;
    let spec = BeatSpec::After(1.0);
    // A scalar on the same beat is the oracle for its sample.
    let level = submit_level(&rig, 0.5, spec);
    let payload = submit_on_clock(&rig, &[4.0], spec);
    let before = freed();

    let mut settled = Vec::new();
    for _ in 0..200 {
        assert_eq!(counted_block(&mut rig), (0, 0));
        settled.extend(outcomes(&mut rig));
        if settled.len() == 2 {
            break;
        }
    }
    let [(first, Outcome::Applied { at }), second] = settled[..] else {
        panic!("not both applied: {settled:?}");
    };
    assert_eq!(first, level);
    assert!(at > start, "on its beat, not at once");
    assert_eq!(
        second,
        (payload, Outcome::Applied { at }),
        "the same sample"
    );
    assert_eq!(output(&rig), 4.0);
    assert_eq!(freed(), before);
    rig.live.reclaim();
    assert_eq!(freed(), before + 1, "the replaced notes");
}

#[test]
fn a_repeating_payload_is_refused_and_freed_off_thread() {
    let mut rig = beat_rig(480.0);
    add_tape(&mut rig);
    rig.live.reclaim();
    let before = freed();
    let spec = BeatSpec::Grid {
        every: 1.0,
        offset: 0.0,
        repeat: true,
    };
    let refused = submit_on_clock(&rig, &[4.0], spec);
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(
        outcomes_of(&mut rig, refused),
        [(refused, Outcome::Refused(Refusal::Unsupported))]
    );
    assert_eq!(output(&rig), 0.25);
    assert_eq!(freed(), before);
    rig.live.reclaim();
    assert_eq!(freed(), before + 1);
}

#[test]
fn a_payload_waiting_on_a_beat_is_refused_and_freed_off_thread_when_an_edit_takes_its_module_or_clock(
) {
    for (removed, refusal) in [
        ("tape", Refusal::TargetGone),
        ("clock", Refusal::TimelineGone),
    ] {
        let mut rig = beat_rig(120.0);
        add_tape(&mut rig);
        rig.live.reclaim();
        let waiting = submit_on_clock(&rig, &[4.0], BeatSpec::After(8.0));
        assert_eq!(counted_block(&mut rig), (0, 0));
        assert_eq!(outcomes_of(&mut rig, waiting), [], "still waiting");
        let before = freed();
        rig.live
            .edit(|change| {
                change.remove(removed);
                Ok(())
            })
            .unwrap();

        assert_eq!(counted_block(&mut rig), (0, 0));
        assert_eq!(freed(), before);
        assert_eq!(
            outcomes_of(&mut rig, waiting),
            [(waiting, Outcome::Refused(refusal))]
        );
        rig.live.reclaim();
        // The refused payload, and the removed tape's notes with it.
        let tape_notes = usize::from(removed == "tape");
        assert_eq!(freed(), before + 1 + tape_notes, "removed: {removed}");
    }
}

/// Submits `[values]` for the tape's notes at `spec` on the clock.
fn submit_on_clock(rig: &Rig, values: &[f32], spec: BeatSpec) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("tape", NOTES).unwrap();
    let clock = publisher
        .control_target("clock", crate::control_request::ControlIndex(0))
        .unwrap();
    let mut request = Request::new(target, notes(values));
    request.when = When::Beat(BeatTime {
        clock: clock.module_idx as u32,
        spec,
        ramp: None,
    });
    rig.live.requests.submit(request).unwrap()
}
