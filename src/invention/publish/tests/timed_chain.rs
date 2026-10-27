//! The control writes a timed edit makes to modules it keeps play from the
//! sample it lands on, mid-block, and never before; one refused, cancelled
//! or expired takes them with it. A write the edit cannot chain (to an
//! undeclared control) refuses the edit with nothing queued, and every
//! block stays allocation-, free- and lock-free.

use super::migrated::block_with_locks_held;
use super::request_beats::{beat_rig, level_change, outcomes_of, render_counted};
use super::Rig;
use crate::control_request::{Outcome, Refusal, Request, RequestId, RequestValue, RtValue, When};
use crate::invention::publish::timed::EditTime;
use crate::invention::runtime::GraphCommandError;
use crate::test_support::dial::LEVEL;
use crate::test_support::host_dial::HostDialFactory;
use crate::ControlValue;

/// Where the edits land: mid-block.
const LANDING: u64 = 64 * 5 + 17;

/// Queues an edit at [`LANDING`] adding an unconnected lfo and writing the
/// dial's level to 0.5 (and `writes` more) as it lands.
fn queue(
    rig: &Rig,
    writes: &[(String, String, ControlValue)],
) -> Result<RequestId, GraphCommandError> {
    let mut change = rig.live.begin();
    change.upsert("lfo", rig.build("lfo", "lfo", serde_json::json!({})));
    let prepared = change.prepare().unwrap();
    let time = EditTime::AtSample(LANDING);
    let mut all = vec![dial_level(0.5)];
    all.extend_from_slice(writes);
    rig.live
        .commit_built_when(prepared, None, time, None, |_| {}, &all)
}

fn dial_level(level: f32) -> (String, String, ControlValue) {
    let level = ControlValue::Number(level);
    ("dial".to_string(), "level".to_string(), level)
}

#[test]
fn the_landing_block_neither_allocates_nor_waits() {
    let mut rig = beat_rig(97.0);
    let id = queue(&rig, &[]).unwrap();
    render_counted(&mut rig, 5);
    let counts = block_with_locks_held(&mut rig, &[]);
    assert_eq!(counts, Ok((0, 0)), "the landing block");
    let after = render_counted(&mut rig, 2);
    let landed = Outcome::Applied { at: LANDING };
    assert_eq!(outcomes_of(&mut rig, id)[0], (id, landed));
    assert!(after.iter().all(|sample| sample - sample.floor() == 0.5));
}

#[test]
fn a_kept_module_s_write_lands_on_the_edit_s_exact_sample() {
    let mut rig = beat_rig(97.0);
    queue(&rig, &[]).unwrap();
    let out = render_counted(&mut rig, 8);
    assert_eq!(level_change(&out, 0.5), Some(LANDING as usize));
}

#[test]
fn a_cancelled_edit_takes_its_writes_with_it() {
    let mut rig = beat_rig(97.0);
    queue(&rig, &[]).unwrap();
    let spare = rig.build("spare", "lfo", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("spare", spare);
            Ok(())
        })
        .unwrap();
    let out = render_counted(&mut rig, 8);
    assert_eq!(level_change(&out, 0.5), None);
    let (outcomes, _) = rig.live.take_outcomes();
    let broken = Outcome::Refused(Refusal::ChainBroken);
    assert!(outcomes.iter().any(|(_, outcome)| *outcome == broken));
}

#[test]
fn a_write_the_edit_cannot_chain_refuses_it_with_nothing_queued() {
    let mut rig = beat_rig(97.0);
    rig.registry.register(HostDialFactory);
    let host_dial = rig.build("fx", "host_dial", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("fx", host_dial);
            Ok(())
        })
        .unwrap();
    let rate = ControlValue::Number(8.0);
    let host = ("fx".to_string(), "rate".to_string(), rate);
    let error = queue(&rig, &[host]).unwrap_err();
    assert!(
        matches!(&error, GraphCommandError::ControlError(message)
        if message.contains("fx.rate")),
        "{error}"
    );
    let too_loud = (
        "dial".to_string(),
        "nope".to_string(),
        ControlValue::Number(1.0),
    );
    assert!(queue(&rig, &[too_loud]).is_err());
    assert!(!rig.live.publisher().lock().unwrap().timed_waiting());
    let out = render_counted(&mut rig, 8);
    assert_eq!(level_change(&out, 0.5), None);
    assert_eq!(rig.module_ids(), ["clock", "dac", "dial", "fx"]);
}

#[test]
fn an_edit_already_due_plays_its_writes_from_the_sample_it_lands_on() {
    let mut rig = beat_rig(97.0);
    render_counted(&mut rig, 8);
    queue(&rig, &[]).unwrap();
    let out = render_counted(&mut rig, 2);
    assert_eq!(level_change(&out, 0.5), Some(0), "with the edit, at once");
    assert_eq!(rig.module_ids(), ["clock", "dac", "dial", "lfo"]);
}

#[test]
fn an_edit_whose_writes_find_no_room_is_refused_with_nothing_queued() {
    let mut rig = beat_rig(97.0);
    // Control writes fill every slot they may take, leaving the edits'.
    let target = {
        let publisher = rig.live.publisher().lock().unwrap();
        publisher.control_target("dial", LEVEL).unwrap()
    };
    let filler = || {
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.25)));
        request.when = When::AtSample(u64::MAX);
        request
    };
    while rig.live.requests.submit(filler()).is_ok() {}

    let error = queue(&rig, &[]).unwrap_err();
    assert!(matches!(error, GraphCommandError::QueueFull), "{error}");
    assert!(!rig.live.publisher().lock().unwrap().timed_waiting());
    assert!(rig.live.take_timed_changes().is_empty(), "never queued");
    render_counted(&mut rig, 8);
    assert_eq!(rig.module_ids(), ["clock", "dac", "dial"]);
    let (outcomes, _) = rig.live.take_outcomes();
    let broken = Outcome::Refused(Refusal::ChainBroken);
    assert!(outcomes.iter().all(|(_, outcome)| *outcome != broken));
}

#[test]
fn a_cancelled_edit_s_writes_give_their_slots_back_at_the_next_block() {
    use super::timed_status::{queue_writing, state_of, write};
    use crate::invention::publish::EditState;
    use crate::ControlWriteIntent;
    let mut rig = Rig::new(super::BASE);
    let id = queue_writing(&rig, 550.0);
    assert_eq!(
        rig.live.requests.watch_slots().taken(),
        1,
        "the chained write"
    );
    write(&rig, "osc1", "frequency", 330.0, ControlWriteIntent::Author);
    assert!(matches!(state_of(&rig, id), EditState::Cancelled(_)));
    rig.render(1);
    assert_eq!(
        rig.live.requests.watch_slots().taken(),
        0,
        "long before the landing"
    );
    let (outcomes, _) = rig.live.take_outcomes();
    let broken = Outcome::Refused(Refusal::ChainBroken);
    assert!(outcomes.iter().any(|(_, outcome)| *outcome == broken));
}
