//! The control writes still pending, as a front door lists them: compact
//! by default, in full on request, one source's alone, and each with what
//! it waits for.

use super::acks::{frequency, on_manual, retune_waiting};
use crate::rpc::{PendingDetail, PendingSource, PendingWait, RequestSource, COMPACT_PENDING_ITEMS};
use crate::ControlWrite;

#[test]
fn a_compact_listing_names_the_oldest_and_counts_the_rest() {
    let (running, pump, _sink) = on_manual();
    let writes: Vec<_> = (0..10)
        .map(|step| {
            let source = [RequestSource::User, RequestSource::Agent][step % 2];
            frequency(300.0 + step as f32, source)
        })
        .collect();
    let acks = running.write_controls(&writes);

    let compact = running.pending_requests(None, PendingDetail::Compact);
    assert_eq!(
        (compact.total, compact.more),
        (10, 10 - COMPACT_PENDING_ITEMS)
    );
    assert_eq!(compact.items.len(), COMPACT_PENDING_ITEMS);
    assert_eq!((compact.by_source.user, compact.by_source.agent), (5, 5));
    // Oldest first, each naming the request its reply did.
    let named: Vec<_> = compact
        .items
        .iter()
        .map(|item| Some(item.request.clone()))
        .collect();
    let replied: Vec<_> = acks
        .controls
        .iter()
        .map(|ack| ack.request.clone())
        .collect();
    assert_eq!(named, replied[..COMPACT_PENDING_ITEMS]);
    let first = &compact.items[0];
    assert_eq!(
        (first.module_id.as_str(), first.key.as_str()),
        ("osc", "frequency")
    );
    assert_eq!(first.requested, 300.0.into());
    assert_eq!(first.waiting, PendingWait::NextBlock);

    let full = running.pending_requests(None, PendingDetail::Full);
    assert_eq!((full.total, full.items.len(), full.more), (10, 10, 0));

    // One source's alone, though every source is still counted.
    let agent = running.pending_requests(Some(PendingSource::Agent), PendingDetail::Full);
    assert_eq!((agent.total, agent.items.len()), (5, 5));
    assert!(agent
        .items
        .iter()
        .all(|item| item.source == PendingSource::Agent));
    assert_eq!(agent.by_source, full.by_source);

    // Applied, nothing is pending.
    pump.block();
    let after = running.pending_requests(None, PendingDetail::Compact);
    assert_eq!((after.total, after.items.len(), after.more), (0, 0, 0));
}

#[test]
fn a_write_kept_over_a_timed_edit_lists_its_copy_waiting_on_the_edit() {
    let (mut running, pump, _sink) = on_manual();
    retune_waiting(&mut running);
    let authored = ControlWrite::new("osc", "frequency", 330.0.into());
    let acks = running.write_controls(&[authored]);
    pump.block();

    // The write applied; its copy waits for the edit to land.
    let listed = running.pending_requests(None, PendingDetail::Full);
    let copy = (listed.items.iter())
        .find(|item| item.key == "frequency" && item.requested == 330.0.into())
        .expect("the copy is listed");
    assert_eq!(copy.waiting, PendingWait::Condition);
    assert_ne!(Some(copy.request.clone()), acks.controls[0].request);

    pump.render(8);
    let landed = running.pending_requests(None, PendingDetail::Full);
    assert!(landed.items.iter().all(|item| item.request != copy.request));
}
