use std::sync::Arc;

use super::*;
use crate::control_request::{FrameScope, Rank, RtValue, Source, Standing};
use crate::test_support::table::{entry, number, TestTable};

fn user_hold(target: &Automation, due: u64) {
    let standing = Standing {
        rank: Rank::new(Source::User, 0),
        due,
    };
    target.cells.holds.take(target.index, standing);
}

#[test]
fn a_declared_entry_writes_its_slot_and_reads_it_back() {
    let control = number(0.25);
    let target = TableTarget(control.clone());
    assert_eq!(target.current(), Some(0.25));
    assert!(target.write(TableValue::Number(0.5)));
    assert_eq!(
        control.cells.automation.written(control.index),
        Some(RtValue::F32(0.5))
    );
    assert_eq!(target.current(), Some(0.5));
}

#[test]
fn a_user_write_holds_a_declared_entry_until_the_next_event() {
    let control = number(0.25);
    let target = TableTarget(control.clone());
    user_hold(&control, 100);
    assert!(!target.write(TableValue::Number(0.5)));
    // An event due at the sample the user's write was due loses to it.
    {
        let _scope = FrameScope::enter(100);
        target.begin(0);
    }
    assert!(!target.write(TableValue::Number(0.5)));
    // The next one, later, takes the control back.
    {
        let _scope = FrameScope::enter(101);
        target.begin(0);
    }
    assert!(target.write(TableValue::Number(0.5)));
}

#[test]
fn targets_are_the_same_when_they_reach_a_control_in_common() {
    let a = number(0.0);
    let b = number(0.0);
    let declared = TableTarget(a.clone());
    assert!(declared.same(&TableTarget(a.clone())));
    assert!(!declared.same(&TableTarget(b.clone())));
    // A development's control reaches its aliases.
    let development = Automation {
        aliases: Some(Arc::from([b.clone(), a.clone()])),
        ..number(0.0)
    };
    assert!(TableTarget(development).same(&declared));
}

#[test]
fn a_table_reads_its_entries_in_place_and_hears_whether_it_is_played() {
    let control = number(0.0);
    let table = TestTable::new(
        3,
        vec![entry(0, 0, 0.5, &control), entry(4, 2, 1.0, &control)],
    );
    let table: &dyn BeatTable = &table;
    assert_eq!(
        (table.id(), table.clock_input(), table.len()),
        (3, "clock", 2)
    );
    let second = table.entry(1).unwrap();
    assert_eq!((second.step, second.ramp_steps), (4, 2));
    assert_eq!(second.value, TableValue::Number(1.0));
    assert!(table.entry(2).is_none());
    table.set_played(super::Player::Clock);
}

#[test]
fn most_ramps_counts_the_most_overlapping_ends_included() {
    assert_eq!(most_ramps([]), 0);
    assert_eq!(most_ramps([(0, 0), (1, 0)]), 0, "no ramp");
    assert_eq!(most_ramps([(0, 4), (1, 1), (2, 1)]), 3);
    // One ending on the step another starts still runs there.
    assert_eq!(most_ramps([(0, 2), (2, 2), (4, 2)]), 2);
    assert_eq!(most_ramps([(0, 2), (3, 2)]), 1);
    assert_eq!(most_ramps((0..65).map(|_| (1, 4))), 65);
    assert_eq!(most_ramps([(u64::MAX, 4), (u64::MAX, 1)]), 2);
}
