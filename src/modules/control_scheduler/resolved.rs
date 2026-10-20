//! The schedule a scheduler plays, as its `schedule` payload carries it.
//!
//! A control thread resolves a schedule against the control surfaces and
//! reserves room for every ramp it can run at once, then hands the whole
//! [`ResolvedSchedule`] to the scheduler by request. The scheduler swaps its
//! contents in between blocks and returns the box holding the ones they
//! replace, to be retired off the audio thread, so a live schedule change
//! allocates, frees and locks nothing there.

use std::sync::atomic::{AtomicU64, Ordering};

use super::schedule::{ResolvedEntry, ScheduleEntry};
use super::ActiveRamp;

/// The next [`ResolvedSchedule::id`]. Zero is never handed out.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A resolved schedule and the ramps it runs.
pub(crate) struct ResolvedSchedule {
    /// Sorted by `at_step`, ties in schedule order.
    pub(super) entries: Vec<ResolvedEntry>,
    /// Ramps in flight. Capacity is reserved for every ramp entry in the
    /// schedule, so pushes never allocate on the audio thread.
    pub(super) ramps: Vec<ActiveRamp>,
    /// Tells this schedule from every other: the scheduler reports the one
    /// it adopted last, so a control thread can tell whether the schedule
    /// it recorded is the one playing.
    pub(super) id: u64,
}

impl ResolvedSchedule {
    /// Wraps resolved `entries`, reserving their ramps. Control thread.
    pub(crate) fn new(entries: Vec<ResolvedEntry>) -> Self {
        let ramp_count = entries.iter().filter(|entry| entry.ramp_steps > 0).count();
        Self {
            entries,
            ramps: Vec::with_capacity(ramp_count),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// The module ids schedule text targets, or `None` when it does not parse.
pub(crate) fn schedule_targets(json: &str) -> Option<Vec<String>> {
    let spec = super::schedule::parse_schedule_json(json).ok()?;
    Some(target_module_ids(&spec))
}

/// Module ids `spec` targets, deduplicated, in first-use order: those its
/// resolved entries write, since a schedule resolves only whole.
pub(super) fn target_module_ids(spec: &[ScheduleEntry]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::with_capacity(spec.len());
    for entry in spec {
        if !ids.contains(&entry.module) {
            ids.push(entry.module.clone());
        }
    }
    ids
}
