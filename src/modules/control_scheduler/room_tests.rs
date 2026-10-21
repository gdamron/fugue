//! A scheduler's room for waiting writes filling up: what gives way, and
//! what must not.

use super::legacy_tests::{block, entry, scheduler, Locking};
use super::*;
use crate::control_request::{while_held, RETRY_CAPACITY};

#[test]
fn a_full_room_never_moves_a_resize_past_the_writes_it_shapes() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    // Degree 2 is there only while the count is 3 or more.
    let mut entries = vec![
        entry(1, "melody", "degree_count", Number(3.0)),
        entry(1, "melody", "degree.2", Number(9.0)),
    ];
    // Values to two other controls, in turn, fill the room for values.
    let room = RETRY_CAPACITY - 8;
    for i in 2..room {
        let control = ["root_note", "seed"][i % 2];
        entries.push(entry(1, "melody", control, Number(40.0 + i as f32)));
    }
    entries.push(entry(1, "melody", "degree_count", Number(1.0)));
    let mut scheduler = scheduler(entries, &directory);
    for gate in [1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    let held = while_held(&locking.melody.table, || block(&mut scheduler, 1.0));
    assert_eq!(held, (0, 0), "step 1");
    block(&mut scheduler, 0.0);
    // The last count found no room and was dropped, counted: had it taken
    // the first count's place, degree 2 would have been refused.
    assert_eq!(locking.melody.degree(2), Ok(9));
    let writes = scheduler.legacy.as_ref().unwrap();
    assert_eq!(writes.refused().count(), 1);
}
