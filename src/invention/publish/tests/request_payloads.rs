//! Payload requests on a live graph: every payload the audio side does not
//! keep is retired and freed by the reclaimer, never dropped in
//! `process_block` (debug builds panic if one is); without retire room the
//! drain leaves payload requests queued, and the edits behind them wait
//! too. Every block here is allocation- and free-free.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::requests::{counted_block, outcomes, try_submit_value};
use super::*;
use crate::control_request::{
    ControlIndex, Outcome, Refusal, RequestId, RequestValue, RtValue, When,
};
use crate::payload::{Payload, Retirer, Shared, MAX_RETIRES_PER_REQUEST};

/// Counts how many of its values were dropped.
#[derive(Clone, Default)]
struct Drops(Arc<AtomicUsize>);

impl Drops {
    fn payload(&self) -> RequestValue {
        RequestValue::Payload(Payload::new(Tracked(self.0.clone())))
    }

    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

struct Tracked(Arc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn submit(rig: &Rig, module_id: &str, value: RequestValue, when: When) -> RequestId {
    try_submit_value(rig, module_id, 0, value, when).unwrap()
}

fn retirer(rig: &mut Rig) -> &mut Retirer {
    &mut rig
        .graph
        .requests
        .as_mut()
        .unwrap()
        .pending
        .outcomes
        .retirer
}

/// Fills the payload retire queue and the retirer's hold, as a stalled
/// reclaimer would, until the drain may take no payload request.
fn saturate_retirer(rig: &mut Rig) {
    while retirer(rig).has_room(MAX_RETIRES_PER_REQUEST) {
        retirer(rig).retire(Box::new(0u8));
    }
}

/// Reclaims until the retirer holds nothing, a block flushing it each time.
fn reclaim_all(rig: &mut Rig) {
    while retirer(rig).held() > 0 {
        rig.live.reclaim();
        rig.render(1);
    }
    rig.live.reclaim();
}

#[test]
fn refused_and_superseded_payloads_are_freed_off_the_audio_thread() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let drops = Drops::default();
    let far = rig.graph.current_sample + 1_000;
    let superseded = submit(&rig, "osc1", drops.payload(), When::AtSample(far));
    submit(&rig, "osc1", drops.payload(), When::AtSample(far));
    let refused = submit(&rig, "osc2", drops.payload(), When::Now);

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(drops.count(), 0, "retired, not dropped");
    assert_eq!(
        outcomes(&mut rig),
        [
            (superseded, Outcome::Superseded),
            (refused, Outcome::Refused(Refusal::Unsupported)),
        ]
    );
    rig.live.reclaim();
    assert_eq!(drops.count(), 2);
}

thread_local! {
    /// The payload the [`keep`] hook's module holds.
    static KEPT: std::cell::RefCell<Option<Shared<Tracked>>> = const { std::cell::RefCell::new(None) };
}

/// A hook whose module keeps a payload and retires the one it replaces.
fn keep(
    _: &mut SignalGraph,
    _: usize,
    _: ControlIndex,
    value: RequestValue,
    retirer: &mut Retirer,
) -> Result<(), Refusal> {
    let RequestValue::Payload(payload) = value else {
        return Err(Refusal::Unsupported);
    };
    let kept = payload.downcast::<Tracked>().map_err(|other| {
        retirer.retire(other);
        Refusal::Unsupported
    })?;
    if let Some(old) = KEPT.with(|slot| slot.borrow_mut().replace(kept)) {
        retirer.retire(old);
    }
    Ok(())
}

#[test]
fn an_applied_payload_is_kept_and_the_one_it_replaces_is_retired() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    rig.graph.request_hook = Some(keep);
    let drops = Drops::default();
    let start = rig.graph.current_sample;
    submit(&rig, "osc1", drops.payload(), When::Now);
    submit(&rig, "osc1", drops.payload(), When::AtSample(start + 9));

    assert_eq!(counted_block(&mut rig), (0, 0));
    rig.live.reclaim();
    assert_eq!(drops.count(), 1, "only the replaced payload is freed");
    drop(KEPT.with(|slot| slot.borrow_mut().take()));
    assert_eq!(drops.count(), 2);
}

#[test]
fn without_retire_room_payload_requests_wait_in_the_queue() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    saturate_retirer(&mut rig);
    let drops = Drops::default();
    let payload = submit(&rig, "osc1", drops.payload(), When::Now);
    // To a module that takes no requests, so both are refused once taken.
    let behind = submit(
        &rig,
        "dac",
        RequestValue::Value(RtValue::F32(0.5)),
        When::Now,
    );

    for _ in 0..2 {
        assert_eq!(counted_block(&mut rig), (0, 0), "waiting for room");
        assert_eq!(outcomes(&mut rig), [], "nothing taken, in order");
    }
    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "taking once flushed");
    let refused = Outcome::Refused(Refusal::Unsupported);
    assert_eq!(outcomes(&mut rig), [(payload, refused), (behind, refused)]);
}

#[test]
fn an_edit_waits_behind_a_payload_request_without_retire_room() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let drops = Drops::default();
    let payload = submit(&rig, "osc2", drops.payload(), When::Now);
    saturate_retirer(&mut rig);
    rig.publish_unreclaimed(|change| change.remove("osc1"));
    let fm = edge("osc2", "audio", "dac", "audio");
    rig.publish_unreclaimed(|change| change.disconnect(fm));

    // The request was submitted first, so both edits wait behind it.
    for _ in 0..2 {
        assert_eq!(counted_block(&mut rig), (0, 0), "waiting for room");
        assert_eq!(rig.generation_and_applied(), (2, 0));
        assert_eq!(outcomes(&mut rig), [], "nothing taken, in order");
    }

    rig.live.reclaim();
    assert_eq!(
        counted_block(&mut rig),
        (0, 0),
        "taking it, then installing"
    );
    let refused = Outcome::Refused(Refusal::Unsupported);
    assert_eq!(outcomes(&mut rig), [(payload, refused)]);
    assert_eq!(rig.generation_and_applied(), (2, 2));
    assert_eq!(rig.module_ids(), ["osc2", "dac"]);
    reclaim_all(&mut rig);
    assert_eq!(drops.count(), 1);
}

#[test]
fn payloads_queued_behind_a_deferred_edit_settle_after_it_installs() {
    use crate::invention::publish::publisher::REQUEST_QUEUE_CAPACITY;

    let mut rig = Rig::new(BASE);
    rig.render(1);
    rig.hold_a_retirement();
    let (generation, applied) = rig.generation_and_applied();
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    rig.publish_unreclaimed(|change| change.disconnect(fm.clone()));
    // Payload requests behind the edit fill what the queue leaves control
    // threads: its slots less the edit's.
    let drops = Drops::default();
    let start = rig.graph.current_sample;
    let mut queued = 0;
    while try_submit_value(
        &rig,
        "osc1",
        0,
        drops.payload(),
        When::AtSample(start + queued),
    )
    .is_ok()
    {
        queued += 1;
    }
    assert_eq!(queued, REQUEST_QUEUE_CAPACITY as u64 - 1);
    let refused_at_the_door = drops.count();
    // Edits still fit, in the slots requests leave them.
    rig.publish_unreclaimed(|change| change.connect(fm).unwrap());
    assert_eq!(rig.generation_and_applied(), (generation + 2, applied));

    for _ in 0..2 {
        assert_eq!(counted_block(&mut rig), (0, 0), "waiting to install");
        assert_eq!(outcomes(&mut rig), [], "nothing taken, so nothing reserved");
    }
    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing both, in order");
    assert_eq!(rig.generation_and_applied(), (generation + 2, applied + 2));
    // Every request's sample has passed after another 5 blocks.
    rig.render(5);
    let settled = outcomes(&mut rig).len();
    assert_eq!(settled as u64, queued);
    reclaim_all(&mut rig);
    assert_eq!(drops.count() - refused_at_the_door, settled);
}
