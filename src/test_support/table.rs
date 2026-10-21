//! A table of automation for tests (see `crate::control_request::BeatTable`):
//! entries on number controls in cells of their own, gated on `clock`.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;

use crate::control_request::{
    Automation, BeatTable, ControlCells, ControlIndex, DeclKind, RtValue, TableEntry, TableTarget,
    TableValue,
};

pub(crate) struct TestTable {
    pub(crate) id: usize,
    pub(crate) plan: u64,
    pub(crate) entries: Vec<TableEntry>,
    pub(crate) played: AtomicBool,
    /// The step the drain last told it.
    pub(crate) step: AtomicI64,
    /// Its entries' ramps that found no room.
    pub(crate) crowded: AtomicU32,
}

impl TestTable {
    pub(crate) fn new(id: usize, entries: Vec<TableEntry>) -> Self {
        Self {
            id,
            plan: 1,
            entries,
            played: AtomicBool::new(false),
            step: AtomicI64::new(-1),
            crowded: AtomicU32::new(0),
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

    fn set_played(&self, played: bool) {
        self.played.store(played, Ordering::Relaxed);
    }

    fn set_step(&self, step: i64) {
        self.step.store(step, Ordering::Relaxed);
    }

    fn retry_legacy(&self) {
        let writes = (self.entries.iter()).find_map(|entry| entry.target.legacy_writes());
        if let Some(writes) = writes {
            writes.retry();
        }
    }

    fn crowded(&self) {
        self.crowded.fetch_add(1, Ordering::Relaxed);
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
