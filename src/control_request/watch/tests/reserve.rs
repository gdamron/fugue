//! Reserved watches (note offs timed in beats) keep a few places of their
//! own that other watches, which may wait without end, cannot take.

use super::*;
use crate::control_request::WAITING_RESERVE;

#[test]
fn a_full_store_leaves_reserved_watches_their_room() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(1);
    watches.insert(request(1, 1, 4.0), 0, &mut store.outcomes);
    assert!(watches.is_full(false) && !watches.is_full(true));
    let reserved = |id| {
        let mut request = request(id, 1, 4.0);
        request.reserved = true;
        request
    };
    for id in 10..10 + WAITING_RESERVE as u64 {
        watches.insert(reserved(id), 0, &mut store.outcomes);
    }
    assert!(log(&outcomes).is_empty(), "each takes the reserve");
    watches.insert(reserved(99), 0, &mut store.outcomes);
    watches.insert(request(100, 1, 4.0), 0, &mut store.outcomes);
    let full = Outcome::Refused(Refusal::PendingFull);
    assert_eq!(log(&outcomes), [(99, full), (100, full)]);
    assert_eq!(watches.len(), 1 + WAITING_RESERVE);
}
