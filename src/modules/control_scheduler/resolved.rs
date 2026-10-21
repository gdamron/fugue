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
    /// The schedule the scheduler plays, re-resolved for a graph change
    /// that rebuilt a target: adopted as new targets for the same entries,
    /// so the step and ramps in flight carry on (see
    /// `ControlScheduler::adopt`).
    pub(super) retarget: bool,
}

impl ResolvedSchedule {
    /// Wraps resolved `entries`, reserving their ramps. Control thread.
    pub(crate) fn new(entries: Vec<ResolvedEntry>) -> Self {
        let ramp_count = entries
            .iter()
            .filter(|entry| entry.ramp_steps() > 0)
            .count();
        Self {
            entries,
            ramps: Vec::with_capacity(ramp_count),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            retarget: false,
        }
    }

    /// The schedule the scheduler plays, re-resolved: `entries` replace its
    /// own as new targets, keeping its place. Control thread.
    pub(crate) fn retargeting(entries: Vec<ResolvedEntry>) -> Self {
        Self {
            retarget: true,
            ..Self::new(entries)
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

/// Whether `next` plans what `entries` do (the same steps, targets,
/// values and ramps, in order), whichever instances each resolved to.
/// Allocation-free: the audio thread checks a retarget with it.
pub(super) fn same_plan(entries: &[ResolvedEntry], next: &[ResolvedEntry]) -> bool {
    entries.len() == next.len()
        && entries.iter().zip(next).all(|(entry, other)| {
            entry.at_step() == other.at_step()
                && entry.ramp_steps() == other.ramp_steps()
                && entry.entry.value == other.entry.value
                && entry.module == other.module
                && entry.control == other.control
        })
}
