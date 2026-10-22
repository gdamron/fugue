//! What a timed edit's submitter learns of it: queued, landed, adopted with
//! the modules it built, expired, or cancelled with the reason.

use super::*;
use crate::control_request::RequestId;
use crate::invention::publish::timed::EditTime;
use crate::invention::publish::{CancelCause, EditState};

/// Blocks before a timed edit at [`LANDING`] has landed.
const BLOCKS: usize = 6;
/// Where the rig's timed edits land, from sample 0.
const LANDING: u64 = 64 * 3 + 9;

/// Queues an edit at [`LANDING`] that rebuilds `osc1` as a square wave, or
/// removes `osc2` when `remove`. As a reload or an edit batch does, its
/// document is the one prepared now, replacing whatever is retained when
/// it is adopted.
fn queue(rig: &Rig, remove: bool, ttl: Option<u64>) -> RequestId {
    let square = rig.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "waveform": "square" }),
    );
    let mut change = rig.live.begin();
    if remove {
        change.remove("osc2");
    } else {
        change.upsert("osc1", square);
    }
    let prepared = change.prepare().unwrap();
    let document = rig.live.state.lock().unwrap().document.clone();
    let retain = move |state: &mut RuntimeState| state.document = document;
    let time = EditTime::AtSample(LANDING);
    rig.live.commit_when(prepared, time, ttl, retain).unwrap()
}

fn state_of(rig: &Rig, id: RequestId) -> EditState {
    rig.live.timed_status(id).unwrap().state
}

#[test]
fn an_edit_reports_queued_landed_and_adopted_with_what_it_built() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    let queued = rig.live.timed_status(id).unwrap();
    assert_eq!(
        (queued.state, queued.sample),
        (EditState::Queued, Some(LANDING))
    );

    rig.render(BLOCKS);
    // The next call taking the publisher adopts it.
    let adopted = rig.live.timed_status(id).unwrap();
    assert_eq!(adopted.state, EditState::Adopted);
    assert_eq!((adopted.sample, adopted.due), (Some(LANDING), None));
    assert_eq!(adopted.built, ["osc1"]);
    assert!(adopted.removed.is_empty());
    let changes: Vec<_> = rig.live.take_timed_changes();
    assert_eq!(changes.len(), 1, "one entry per edit, at its latest");
    assert_eq!(changes[0].state, EditState::Adopted);
    assert!(rig.live.take_timed_changes().is_empty());

    let committed = rig.live.take_timed_committed(id).unwrap();
    assert_eq!(committed.stopped, ["osc1"]);
    assert_eq!(committed.started.len(), 1);
    assert!(rig.live.take_timed_committed(id).is_none(), "taken once");
}

#[test]
fn an_edit_whose_ttl_passes_first_reports_expired() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, true, Some(64));
    rig.render(BLOCKS);
    assert_eq!(state_of(&rig, id), EditState::Expired);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
}

#[test]
fn a_structural_commit_cancels_a_waiting_edit_and_says_so() {
    let mut rig = Rig::new(BASE);
    let id = queue(&rig, false, None);
    rig.live
        .connect(edge("osc1", "audio", "osc2", "frequency_mod"))
        .unwrap();
    assert_eq!(
        state_of(&rig, id),
        EditState::Cancelled(CancelCause::Commit)
    );
    rig.render(BLOCKS);
    // Refused `TargetGone` once due, still reported as cancelled.
    assert_eq!(
        state_of(&rig, id),
        EditState::Cancelled(CancelCause::Commit)
    );
    let osc1 = &rig.live.state.lock().unwrap().modules["osc1"];
    assert_eq!(
        osc1.config.get("waveform"),
        Some(&serde_json::json!("sine"))
    );
}

/// An edit adopted before its outcome arrives is announced adopted once:
/// the sample it landed on fills in quietly, for reads.
#[test]
fn a_landing_sample_after_adoption_is_not_announced_again() {
    use crate::control_request::Outcome;
    use crate::invention::publish::timed_status::Ledger;
    let mut ledger = Ledger::default();
    let id = RequestId(7);
    ledger.queued(id, None, None);
    ledger.adopted(id, Committed::default());
    let announced: Vec<_> = ledger.take_changes().into_iter().map(|s| s.state).collect();
    assert_eq!(announced, [EditState::Adopted]);

    ledger.outcome(id, Outcome::Applied { at: 4_096 });
    assert!(ledger.take_changes().is_empty(), "announced once");
    let read = ledger.status(id).unwrap();
    assert_eq!((read.state, read.sample), (EditState::Adopted, Some(4_096)));
}
