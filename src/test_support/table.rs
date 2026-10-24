//! A table of automation for tests (see `crate::control_request::BeatTable`):
//! entries on number controls in cells of their own, gated on `clock`.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use crate::control_request::{
    Automation, BeatTable, ControlCells, ControlIndex, DeclKind, Player, RtValue, TableEntry,
    TableTarget, TableValue,
};

pub(crate) struct TestTable {
    pub(crate) id: usize,
    pub(crate) plan: u64,
    pub(crate) entries: Vec<TableEntry>,
    pub(crate) played: AtomicBool,
    /// The step the drain last told it, and whether it began there.
    pub(crate) step: AtomicI64,
    pub(crate) began: AtomicBool,
}

impl TestTable {
    pub(crate) fn new(id: usize, entries: Vec<TableEntry>) -> Self {
        Self {
            id,
            plan: 1,
            entries,
            played: AtomicBool::new(false),
            step: AtomicI64::new(-1),
            began: AtomicBool::new(false),
        }
    }
}

impl BeatTable for TestTable {
    fn id(&self) -> usize {
        self.id
    }

    fn clock_input(&self) -> &str {
        "clock"
    }

    fn plan(&self) -> u64 {
        self.plan
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn entry(&self, index: usize) -> Option<&TableEntry> {
        self.entries.get(index)
    }

    fn set_played(&self, player: Player) {
        self.played
            .store(player == Player::Clock, Ordering::Relaxed);
    }

    fn set_step(&self, step: i64, began: bool) {
        self.step.store(step, Ordering::Relaxed);
        self.began.store(began, Ordering::Relaxed);
    }
}

/// A number control holding `value`, in cells of its own, as automation
/// writes it.
pub(crate) fn number(value: f32) -> Automation {
    Automation {
        cells: Arc::new(ControlCells::new([RtValue::F32(value)])),
        index: ControlIndex(0),
        kind: DeclKind::Number {
            min: f32::MIN,
            max: f32::MAX,
        },
        clamp: None,
        event: false,
        aliases: None,
    }
}

/// `value` written to `target` at step `step`, or glided to over
/// `ramp_steps` steps.
pub(crate) fn entry(step: u64, ramp_steps: u64, value: f32, target: &Automation) -> TableEntry {
    TableEntry {
        step,
        ramp_steps,
        value: TableValue::Number(value),
        target: TableTarget::Declared(target.clone()),
    }
}
