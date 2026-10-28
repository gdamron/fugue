//! Note offs sent while note ons fill a live graph's request queue and
//! pending store still end their notes: the control is declared reserved,
//! so they take room in the queue only reserved requests may take, and in
//! the full store the places of note ons due later, and the note ons
//! queued ahead of them are refused rather than leave them stuck behind.
//! So across an edit's drain. More of them than any one instrument holds
//! voices all apply, and the note is heard ending at the DAC, in blocks
//! that allocate and free nothing.

use super::requests::{counted_block, outcomes};
use super::sample_instrument::{rig, set, TOL};
use super::*;
use crate::control_request::{
    ControlIndex, Outcome, Refusal, Request, RequestValue, RtValue, When,
};
use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
use serde_json::json;
use std::sync::atomic::Ordering;

const NOTE_ON: ControlIndex = ControlIndex(1);

/// Note offs sent at once: 60's, which sounds, and more after it, as many
/// as six instruments' full voices release.
const OFFS: usize = 6 * crate::modules::sample_instrument::MAX_VOICES;

/// Releases 60 and the notes after it, `OFFS` in all, as players would.
fn note_offs(rig: &Rig) {
    for n in 0..OFFS {
        let note = (60 + n) % 128;
        set(rig, "note_off", (note as f32).into()).unwrap();
    }
}

/// A note on for 60 at `at`, as a scheduler might queue it, or `None`
/// when the queue has no room for it.
fn note_on(rig: &Rig, at: u64) -> Option<()> {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("inst", NOTE_ON).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(60.0)));
    request.when = When::AtSample(at);
    request.event = true;
    rig.live.requests.submit(request).ok().map(drop)
}

/// A rig whose instrument plays into the DAC, note 60 sounding, beside
/// two oscillators heard nowhere, `osc1` and `osc2`, for edits to rewire.
fn sounding(dir: &std::path::Path) -> Rig {
    let mut rig = rig(dir);
    let (osc1, osc2) = (
        rig.build("osc1", "oscillator", json!({})),
        rig.build("osc2", "oscillator", json!({})),
    );
    let wire = |change: &mut GraphChange| {
        change.upsert("osc1", osc1);
        change.upsert("osc2", osc2);
        change.connect(edge("inst", "audio_left", "dac", "audio"))
    };
    rig.live.edit(wire).unwrap();
    set(&rig, "note_on", 60.0.into()).unwrap();
    rig.render(1);
    outcomes(&mut rig);
    rig
}

/// [`sounding`], its pending store full of note ons due long after, and
/// `queued` more in the queue behind them.
fn flooded(dir: &std::path::Path, queued: usize) -> Rig {
    let mut rig = sounding(dir);
    let far = rig.graph.current_sample + 10_000_000;
    let mut next = 0;
    for _ in 0..PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY {
        for _ in 0..REQUEST_QUEUE_CAPACITY {
            note_on(&rig, far + next).unwrap();
            next += 1;
        }
        assert_eq!(counted_block(&mut rig), (0, 0), "filling");
    }
    for _ in 0..queued {
        note_on(&rig, far + next).unwrap();
        next += 1;
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());
    outcomes(&mut rig);
    rig
}

/// One block, allocation- and free-free, and what the DAC played of it.
fn heard_block(rig: &mut Rig) -> Vec<f32> {
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    let graph = &mut rig.graph;
    let ((), allocs, frees) = allocator_events(|| graph.process_block(&mut left, &mut right));
    assert_eq!((allocs, frees), (0, 0));
    left.to_vec()
}

/// The note ended: its last frame is silent, and so is the next block.
fn ended(rig: &mut Rig, heard: &[f32]) {
    assert!(heard[63].abs() < TOL, "still sounding: {}", heard[63]);
    let after = rig.render(1);
    assert!(after.iter().all(|level| level.abs() < TOL), "{after:?}");
}

/// Of the outcomes so far, how many were each.
fn tally(rig: &mut Rig) -> (usize, usize) {
    let settled = outcomes(rig);
    let applied = (settled.iter())
        .filter(|(_, outcome)| matches!(outcome, Outcome::Applied { .. }))
        .count();
    let refused = (settled.iter())
        .filter(|(_, outcome)| *outcome == Outcome::Refused(Refusal::PendingFull))
        .count();
    assert_eq!(applied + refused, settled.len(), "{settled:?}");
    (applied, refused)
}

#[test]
fn note_offs_end_their_notes_while_note_ons_fill_the_queue_and_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = flooded(dir.path(), REQUEST_QUEUE_CAPACITY);
    assert!(note_on(&rig, rig.graph.current_sample).is_none(), "full");

    note_offs(&rig);
    // The note ons ahead of them are taken and refused for want of room,
    // as many as a block takes; they are taken the block after, each in
    // the place of a note on due later.
    heard_block(&mut rig);
    assert_eq!(tally(&mut rig), (0, REQUEST_QUEUE_CAPACITY));
    let heard = heard_block(&mut rig);
    ended(&mut rig, &heard);
    assert_eq!(tally(&mut rig), (OFFS, OFFS));
}

#[test]
fn note_offs_held_across_an_edit_drain_end_their_notes() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = flooded(dir.path(), 100);
    note_offs(&rig);
    let spare = rig.build("spare", "oscillator", json!({}));
    rig.live
        .edit(|change: &mut GraphChange| {
            change.upsert("spare", spare);
            Ok(())
        })
        .unwrap();

    let heard = heard_block(&mut rig);
    assert!(rig.module_ids().contains(&"spare".to_string()), "installed");
    ended(&mut rig, &heard);
    assert_eq!(tally(&mut rig), (OFFS, 100 + OFFS));
}

/// [`sounding`], its retirement held so edits stall, the queue full of
/// note ons due long after and of edits behind them, but for the room
/// left for reserved requests.
fn stalled(dir: &std::path::Path) -> Rig {
    use crate::invention::publish::publisher::{EDIT_RESERVE, RESERVED_ROOM};
    let mut rig = sounding(dir);
    rig.hold_a_retirement();
    let far = rig.graph.current_sample + 10_000_000;
    for n in 0..REQUEST_QUEUE_CAPACITY as u64 {
        note_on(&rig, far + n).unwrap();
    }
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    let mut edits = 0;
    loop {
        let mut publisher = rig.live.publisher().lock().unwrap();
        let mut change = rig.live.change_on(&publisher);
        match edits % 2 {
            0 => change.disconnect(fm.clone()),
            _ => change.connect(fm.clone()).unwrap(),
        }
        if publisher.publish(change.prepare().unwrap()).is_err() {
            break;
        }
        edits += 1;
    }
    assert_eq!(edits, EDIT_RESERVE - RESERVED_ROOM);
    rig
}

/// Renders until what is queued has applied, and counts the note offs
/// that did.
fn applied_after_stall(rig: &mut Rig) -> (Vec<f32>, usize) {
    let mut heard = Vec::new();
    let mut applied = 0;
    let held = rig.live.held_flush();
    for _ in 0..64 {
        rig.live.reclaim();
        // As the reclaimer's thread would, between blocks.
        held.tick();
        heard = rig.render(1);
        applied += (outcomes(rig).into_iter())
            .filter(|(_, outcome)| matches!(outcome, Outcome::Applied { .. }))
            .count();
    }
    (heard, applied)
}

#[test]
fn a_note_off_finds_room_in_a_queue_full_of_requests_and_stalled_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = stalled(dir.path());
    note_offs(&rig);
    let (heard, applied) = applied_after_stall(&mut rig);
    assert!(heard.iter().all(|level| level.abs() < TOL), "{heard:?}");
    assert_eq!(applied, OFFS, "every note off");
}

/// As many note offs as forty full instruments release at once all find
/// room behind a full queue and stalled edits, and apply once the
/// installs go on.
#[test]
fn every_voice_of_many_instruments_ends_behind_stalled_edits() {
    const VOICES: usize = 40 * crate::modules::sample_instrument::MAX_VOICES;
    let dir = tempfile::tempdir().unwrap();
    let mut rig = stalled(dir.path());
    for n in 0..VOICES {
        let note = (60 + n) % 128;
        set(&rig, "note_off", (note as f32).into()).unwrap();
    }
    let (heard, applied) = applied_after_stall(&mut rig);
    assert!(heard.iter().all(|level| level.abs() < TOL), "{heard:?}");
    assert_eq!(applied, VOICES, "every note off");
}

/// More note offs than the queue has room for, behind stalled edits: those
/// past the room are held until it frees, and every one applies. Past the
/// backlog too, one is refused.
#[test]
fn note_offs_past_the_queue_wait_for_room_and_all_end() {
    use crate::control_request::BACKLOG_CAPACITY;
    use crate::invention::publish::publisher::RESERVED_ROOM;
    let dir = tempfile::tempdir().unwrap();
    let mut rig = stalled(dir.path());
    let offs = RESERVED_ROOM + BACKLOG_CAPACITY;
    for n in 0..offs {
        let note = (60 + n) % 128;
        set(&rig, "note_off", (note as f32).into()).unwrap();
    }
    assert!(
        set(&rig, "note_off", 60.0.into()).is_err(),
        "past the backlog"
    );
    let (heard, applied) = applied_after_stall(&mut rig);
    assert!(heard.iter().all(|level| level.abs() < TOL), "{heard:?}");
    assert_eq!(applied, offs, "every note off");
}

/// While note offs are held, one submitted after them never overtakes
/// them, though the audio thread has freed room since: what is held is
/// queued first, so every note off applies in the order it was sent.
#[test]
fn nothing_overtakes_a_held_note_off() {
    use crate::invention::publish::publisher::RESERVED_ROOM;
    let dir = tempfile::tempdir().unwrap();
    let mut rig = stalled(dir.path());
    let offs = RESERVED_ROOM + 2;
    for n in 0..offs - 1 {
        set(&rig, "note_off", (((60 + n) % 128) as f32).into()).unwrap();
    }
    assert!(rig.live.requests.holding().load(Ordering::Acquire), "held");
    // The audio thread frees room, which nothing has filled from the
    // backlog yet.
    rig.live.reclaim();
    rig.render(1);
    assert!(rig.live.requests.holding().load(Ordering::Acquire));
    set(&rig, "note_off", 61.0.into()).unwrap();
    assert!(
        !rig.live.requests.holding().load(Ordering::Acquire),
        "queued"
    );
    let mut applied = Vec::new();
    for _ in 0..64 {
        rig.live.reclaim();
        rig.render(1);
        applied.extend(
            (outcomes(&mut rig).into_iter())
                .filter(|(_, outcome)| matches!(outcome, Outcome::Applied { .. }))
                .map(|(id, _)| id),
        );
    }
    assert_eq!(applied.len(), offs, "every note off");
    assert!(applied.is_sorted(), "in the order sent");
}

/// A note off held while a timed edit waits to land is tagged with the
/// graph the edit replaces. The edit lands and is adopted, and the queue
/// drains, before anything queues what is held: the note off still
/// reaches the instrument the edit keeps, and its note ends.
#[test]
fn a_note_off_held_across_a_timed_edit_still_ends_its_note() {
    use crate::invention::publish::timed::EditTime;
    let dir = tempfile::tempdir().unwrap();
    let mut rig = sounding(dir.path());
    let spare = rig.build("spare", "oscillator", json!({}));
    let mut change = rig.live.begin();
    change.upsert("spare", spare);
    let at = EditTime::AtSample(rig.graph.current_sample + 64);
    let edit = (rig.live)
        .commit_when(change.prepare().unwrap(), at, None, |state| {
            state.document = None;
        })
        .unwrap();
    let far = rig.graph.current_sample + 10_000_000;
    let mut next = 0;
    while note_on(&rig, far + next).is_some() {
        next += 1;
    }
    // Note offs for a note not sounding, until one is held.
    let holding = rig.live.requests.holding();
    while !holding.load(Ordering::Acquire) {
        set(&rig, "note_off", 61.0.into()).unwrap();
    }
    set(&rig, "note_off", 60.0.into()).unwrap();
    for _ in 0..8 {
        rig.render(1);
        rig.live.adopt_landed();
    }
    assert!(rig.module_ids().contains(&"spare".to_string()), "landed");
    let landed = outcomes(&mut rig).into_iter().find(|(id, _)| *id == edit);
    assert!(
        matches!(landed, Some((_, Outcome::Applied { .. }))),
        "{landed:?}"
    );
    assert!(
        holding.load(Ordering::Acquire),
        "nothing queued what is held"
    );
    rig.live.held_flush().tick();
    let heard = rig.render(2);
    assert!(
        heard[64..].iter().all(|level| level.abs() < TOL),
        "{heard:?}"
    );
    let gone = Outcome::Refused(Refusal::TargetGone);
    assert!(!outcomes(&mut rig)
        .iter()
        .any(|(_, outcome)| *outcome == gone));
}
