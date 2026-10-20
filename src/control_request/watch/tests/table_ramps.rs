//! Tables' ramps on a scripted timeline: written through each entry's own
//! target, dropped while a higher tier holds the control, replaced by the
//! table's next entry on it, ended with their table, capped apart from
//! requests' ramps, and running without allocating or freeing.

use super::ramps::{control, RampHost};
use super::*;
use crate::control_request::automation::Automation;
use crate::control_request::hold::Rank;
use crate::control_request::ramps::{BeatRamp, EntryKey, Ramps};
use crate::control_request::request::Source;
use crate::test_support::table::{entry, number, TestTable};

/// The table's module, and its id.
const TABLE: (usize, usize) = (2, 7);
const PLAN: u64 = 1;

/// A host whose module 2 plays a table of an entry on each of `controls`,
/// at step 0, ramping to 1 over two steps.
fn host(controls: &[&Automation]) -> RampHost {
    let entries = controls.iter().map(|c| entry(0, 2, 1.0, c)).collect();
    let mut host = RampHost::new(0.0);
    host.table = Some(TestTable::new(TABLE.1, entries));
    host
}

/// Plays to sample 127, where the clock's next sample is at beat 1, and
/// starts entry `entry`'s ramp there from its control's value to 1 over
/// two beats.
fn begin(ramps: &mut Ramps, host: &mut RampHost, entry: usize) -> bool {
    host.play(127u64.saturating_sub(host.now));
    let table = host.table.as_ref().unwrap();
    let from = table.entries[entry].target.current().unwrap();
    let ramp = BeatRamp::new(from, 1.0, 1.0, 2.0, 0);
    let key = EntryKey {
        module: TABLE.0,
        table: TABLE.1,
        plan: PLAN,
        entry,
    };
    ramps.start_entry(key, 0, ramp, &*host)
}

/// Steps the ramps at each segment start until sample `until`, recording
/// what `watched` holds after each step.
fn run(
    ramps: &mut Ramps,
    host: &mut RampHost,
    store: &mut PendingStore,
    until: u64,
    watched: &Automation,
    seen: &mut Vec<(u64, f32)>,
) {
    while host.now < until {
        let arrives = ramps.step(INSTALLED, host, &mut store.outcomes.retirer);
        if let Some(value) = watched.current() {
            if seen.last().is_none_or(|&(_, last)| last != value) {
                seen.push((host.now, value));
            }
        }
        host.play(arrives.unwrap_or(64).min(64).min(until - host.now));
    }
}

fn hold(control: &Automation) {
    let standing = Standing {
        rank: Rank::new(Source::User, 0),
        due: 0,
    };
    control.cells.holds.take(control.index, standing);
}

#[test]
fn a_table_s_ramp_writes_its_slot_and_arrives_exactly_on_its_end_sample() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let level = number(0.25);
    let mut host = host(&[&level]);
    assert!(begin(&mut ramps, &mut host, 0));
    let mut seen = Vec::with_capacity(64);
    run(&mut ramps, &mut host, &mut store, 1024, &level, &mut seen);
    assert!(ramps.is_empty(), "it ends silently once it arrives");
    assert_eq!(seen[0], (127, 0.25));
    assert_eq!(*seen.last().unwrap(), (383, 1.0), "it lands on beat 3");
    assert!(host.writes.is_empty(), "nothing goes through requests");
}

#[test]
fn a_table_s_ramp_a_higher_tier_holds_is_dropped_until_the_next_entry() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let level = number(0.0);
    let mut host = host(&[&level]);
    begin(&mut ramps, &mut host, 0);
    let mut seen = Vec::with_capacity(64);
    run(&mut ramps, &mut host, &mut store, 200, &level, &mut seen);
    hold(&level);
    let before = level.current();
    run(&mut ramps, &mut host, &mut store, 1024, &level, &mut seen);
    assert!(ramps.is_empty());
    assert_eq!(level.current(), before, "nothing written while held");
    // The table's next entry takes the control back.
    level.begin(0);
    assert!(begin(&mut ramps, &mut host, 0));
}

#[test]
fn the_table_s_next_entry_on_a_control_replaces_its_ramp_only() {
    let mut ramps = Ramps::new(4);
    let (a, b) = (number(0.0), number(0.0));
    let mut host = host(&[&a, &b, &a]);
    begin(&mut ramps, &mut host, 0);
    begin(&mut ramps, &mut host, 1);
    assert_eq!(ramps.running(), 2);
    let next = host.table.as_ref().unwrap().entries[2].target.clone();
    ramps.supersede_entry(&next, &host);
    assert_eq!(ramps.running(), 1, "b's ramp runs on");
    // Starting entry 2's ramp on a replaces any left on it.
    begin(&mut ramps, &mut host, 2);
    assert_eq!(ramps.running(), 2);
}

#[test]
fn a_table_s_ramps_end_with_its_plan_or_its_table_and_move_with_it() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let level = number(0.0);
    let mut host = host(&[&level]);
    begin(&mut ramps, &mut host, 0);
    ramps.end_table(TABLE.1 + 1);
    assert_eq!(ramps.running(), 1, "another table's end leaves it");
    ramps.end_table(TABLE.1);
    assert_eq!(ramps.running(), 0);

    // Its module moved: found again at 3, it writes on from there.
    begin(&mut ramps, &mut host, 0);
    ramps.retable(TABLE.1, 3, 0);
    ramps.step(INSTALLED, &mut host, &mut store.outcomes.retirer);
    assert!(ramps.is_empty(), "module 3 plays no table here");

    // Another table, found where its module was, is not its own.
    begin(&mut ramps, &mut host, 0);
    host.table.as_mut().unwrap().id = TABLE.1 + 1;
    ramps.step(INSTALLED, &mut host, &mut store.outcomes.retirer);
    assert!(ramps.is_empty(), "it plays another table");
    host.table.as_mut().unwrap().id = TABLE.1;

    // Another plan replaced the entries: it ends without a write.
    begin(&mut ramps, &mut host, 0);
    host.table.as_mut().unwrap().plan = PLAN + 1;
    let before = level.current();
    ramps.step(INSTALLED, &mut host, &mut store.outcomes.retirer);
    assert!(ramps.is_empty());
    assert_eq!(level.current(), before);
}

#[test]
fn tables_and_requests_ramps_are_capped_apart() {
    let mut ramps = Ramps::new(2);
    let controls = [number(0.0), number(0.0), number(0.0)];
    let mut host = host(&[&controls[0], &controls[1], &controls[2]]);
    assert!(begin(&mut ramps, &mut host, 0));
    assert!(begin(&mut ramps, &mut host, 1));
    assert!(!begin(&mut ramps, &mut host, 2), "the tables' room is full");
    assert!(ramps.has_room(1), "requests' room is their own");
    let standing = Standing::of(&request(1, 1, 0.0), host.now).unwrap();
    for _ in 0..2 {
        let ramp = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 0);
        ramps.start((control(), 0), standing, ramp, &host);
        ramps.supersede(&control(), &host);
    }
    assert!(!ramps.has_room(0));
    assert!(!begin(&mut ramps, &mut host, 2));
}

#[test]
fn running_a_table_s_ramps_neither_allocates_nor_frees() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let (a, b) = (number(0.0), number(0.5));
    let mut host = host(&[&a, &b, &a]);
    let mut seen = Vec::with_capacity(1024);
    let ((), allocs, frees) = allocator_events(|| {
        begin(&mut ramps, &mut host, 0);
        begin(&mut ramps, &mut host, 1);
        run(&mut ramps, &mut host, &mut store, 200, &a, &mut seen);
        let next = host.table.as_ref().unwrap().entries[2].target.clone();
        ramps.supersede_entry(&next, &host);
        drop(next);
        run(&mut ramps, &mut host, &mut store, 1024, &a, &mut seen);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(ramps.is_empty());
}
