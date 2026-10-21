use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::control_request::EventCounter;
use crate::control_request::{FrameScope, Rank, RtValue, Source, Standing};
use crate::test_support::table::{entry, number, TestTable};
use crate::{ControlMeta, ControlValue};

/// A legacy surface holding one number, `level`.
#[derive(Default)]
struct Knob(AtomicU32);

impl ControlSurface for Knob {
    fn controls(&self) -> Vec<ControlMeta> {
        Vec::new()
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        (key == "level")
            .then(|| ControlValue::Number(f32::from_bits(self.0.load(Ordering::Relaxed))))
            .ok_or_else(|| key.to_string())
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match (key, value) {
            ("level", ControlValue::Number(number)) => {
                self.0.store(number.to_bits(), Ordering::Relaxed);
                Ok(())
            }
            _ => Err(key.to_string()),
        }
    }
}

fn legacy(surface: &Arc<Knob>, control: &str) -> TableTarget {
    TableTarget::legacy(
        surface.clone(),
        control.to_string(),
        Arc::new(EventCounter::new()),
    )
}

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
    let target = TableTarget::Declared(control.clone());
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
    let target = TableTarget::Declared(control.clone());
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
fn a_legacy_entry_writes_through_its_surface_with_no_holds() {
    let knob = Arc::new(Knob::default());
    let target = legacy(&knob, "level");
    target.begin(0);
    assert!(target.write(TableValue::Number(0.75)));
    assert_eq!(target.current(), Some(0.75));
    // A refused write is the setter's own: nothing holds it.
    assert!(legacy(&knob, "missing").write(TableValue::Bool(true)));
    assert_eq!(legacy(&knob, "missing").current(), None);
}

#[test]
fn targets_are_the_same_when_they_reach_a_control_in_common() {
    let a = number(0.0);
    let b = number(0.0);
    let declared = TableTarget::Declared(a.clone());
    assert!(declared.same(&TableTarget::Declared(a.clone())));
    assert!(!declared.same(&TableTarget::Declared(b.clone())));
    // A development's control reaches its aliases.
    let development = Automation {
        aliases: Some(Arc::from([b.clone(), a.clone()])),
        ..number(0.0)
    };
    assert!(TableTarget::Declared(development).same(&declared));

    let knob = Arc::new(Knob::default());
    let other = Arc::new(Knob::default());
    assert!(legacy(&knob, "level").same(&legacy(&knob, "level")));
    assert!(!legacy(&knob, "level").same(&legacy(&other, "level")));
    assert!(!legacy(&knob, "level").same(&legacy(&knob, "other")));
    assert!(!legacy(&knob, "level").same(&declared));
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
    table.set_played(true);
}
