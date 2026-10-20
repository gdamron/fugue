//! The schedule a scheduler plays, as its `schedule` payload carries it.
//!
//! A control thread resolves a schedule against the control surfaces and
//! reserves room for every ramp it can run at once, then hands the whole
//! [`ResolvedSchedule`] to the scheduler by request. The scheduler swaps its
//! contents in between blocks and returns the box holding the ones they
//! replace, to be retired off the audio thread, so a live schedule change
//! allocates, frees and locks nothing there.

use super::schedule::ResolvedEntry;
use super::ActiveRamp;

/// A resolved schedule and the ramps it runs.
pub(crate) struct ResolvedSchedule {
    /// Sorted by `at_step`, ties in schedule order.
    pub(super) entries: Vec<ResolvedEntry>,
    /// Ramps in flight. Capacity is reserved for every ramp entry in the
    /// schedule, so pushes never allocate on the audio thread.
    pub(super) ramps: Vec<ActiveRamp>,
}

impl ResolvedSchedule {
    /// Wraps resolved `entries`, reserving their ramps. Control thread.
    pub(crate) fn new(entries: Vec<ResolvedEntry>) -> Self {
        let ramp_count = entries.iter().filter(|entry| entry.ramp_steps > 0).count();
        Self {
            entries,
            ramps: Vec::with_capacity(ramp_count),
        }
    }
}

/// Module ids `entries` target, deduplicated, in first-use order.
pub(super) fn target_module_ids(entries: &[ResolvedEntry]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::with_capacity(entries.len());
    for entry in entries {
        if !ids.contains(&entry.module) {
            ids.push(entry.module.clone());
        }
    }
    ids
}
