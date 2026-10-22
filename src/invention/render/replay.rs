//! Replaying a live session's request log in an offline render.
//!
//! Each logged request applies at its recorded sample, by splitting the
//! render there, exactly as the live drain split the block it applied in,
//! and through the same apply a live request takes (automation still
//! waiting first, then the module's declared control). A request that
//! applied on its clock's beat latches the clock first, as the live watch
//! did, so a tempo it changes there takes effect from the same sample. (A
//! watch refused or overridden on its beat latched its clock live,
//! unlogged: a late
//! request changing that clock's tempo in the same sample would then take
//! effect a sample apart. Rare enough to leave.)
//! Replay is by sample:
//! a render of the invention as loaded, at the session's sample rate and
//! block grid, then reproduces what played (see
//! [`RequestLog::is_exact`](crate::RequestLog::is_exact)). A log entry's
//! musical time is kept for the record; replaying against a changed tempo
//! by beats is not done here.
//!
//! A replayed request holds its control above automation as it did live
//! (`crate::control_request::hold`), from the sample it was due at, so a
//! scheduler entry is held off, or not, as it was. The log does not keep a
//! request's tier: every tier above automation holds against it alike, a
//! request that lost to a higher rank never applied, so is not logged, and
//! an automation-tier request (which takes a control back from a hold) is
//! counted unlogged, not replayed as a hold.

use indexmap::IndexMap;

use crate::control_request::{take_automation, ControlIndex, RtValue};
use crate::invention::graph::SignalGraph;
use crate::invention::runtime::ControlSurfaceInstance;
use crate::{LoggedBeat, RequestLog};

/// A logged request resolved against the render's surfaces.
struct Due {
    sample: u64,
    /// The sample it was due at, which its hold counts from.
    due: u64,
    module_id: String,
    control: ControlIndex,
    value: RtValue,
    /// The clock it applied on the beat of, which it latches first.
    latch: Option<String>,
}

/// The requests a render still has to apply, in log order.
#[derive(Default)]
pub(super) struct Replay {
    due: Vec<Due>,
    next: usize,
    /// Requests the render's graph refused (it would not have logged them).
    pub(super) refused: usize,
}

impl Replay {
    /// Resolves every request in `log` to a declared control of a module
    /// in `surfaces`, or fails naming the first that is not one, or the
    /// first that is out of time order.
    pub(super) fn resolve(
        log: &RequestLog,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
    ) -> Result<Self, String> {
        let samples = log.requests.iter().map(|request| request.sample);
        if let Some(at) = samples
            .clone()
            .zip(samples.skip(1))
            .position(|(a, b)| b < a)
        {
            return Err(format!("logged request {} is out of time order", at + 1));
        }
        let due = log
            .requests
            .iter()
            .map(|request| {
                let control = surfaces
                    .get(&request.module_id)
                    .and_then(|surface| surface.declaration(&request.key))
                    .ok_or_else(|| {
                        format!(
                            "the logged request for '{}' of '{}' names no declared control",
                            request.key, request.module_id
                        )
                    })?
                    .index;
                // Live latches a clock for a watch applied on its beat, not
                // for one that applied late.
                let latch = match (&request.beat, request.due) {
                    (
                        Some(LoggedBeat::After { clock, .. } | LoggedBeat::Grid { clock, .. }),
                        None,
                    ) => Some(clock.clone()),
                    _ => None,
                };
                Ok(Due {
                    sample: request.sample,
                    due: request.due.unwrap_or(request.sample).min(request.sample),
                    module_id: request.module_id.clone(),
                    control,
                    value: request.value.into(),
                    latch,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            due,
            next: 0,
            refused: 0,
        })
    }

    /// Fails naming the first clock a logged beat names that keeps no beat
    /// timeline in `graph`: its latch would silently do nothing.
    pub(super) fn check_clocks(&self, graph: &mut SignalGraph) -> Result<(), String> {
        for clock in self.due.iter().filter_map(|due| due.latch.as_ref()) {
            let module = graph.modules.get_mut(clock);
            if module.is_none_or(|module| module.module_mut().timeline_mut().is_none()) {
                return Err(format!(
                    "the logged beat's clock '{clock}' is not a clock here"
                ));
            }
        }
        Ok(())
    }

    /// Applies every request due at `graph`'s current sample (one already
    /// past applies now, as a late request would), and returns how many of
    /// the next `frames` frames to render before the next is due.
    pub(super) fn apply_due(&mut self, graph: &mut SignalGraph, frames: usize) -> usize {
        let now = graph.current_sample;
        while let Some(due) = self.due.get(self.next).filter(|due| due.sample <= now) {
            let clock = due
                .latch
                .as_ref()
                .and_then(|clock| graph.modules.get_mut(clock));
            if let Some(clock) = clock {
                // As the live watch does: automation written before this
                // sample first, then the latch.
                take_automation(clock.module_mut());
                if let Some(timeline) = clock.module_mut().timeline_mut() {
                    timeline.latch();
                }
            }
            if graph
                .apply_control_due(&due.module_id, due.control, due.value, due.due)
                .is_err()
            {
                self.refused += 1;
            }
            self.next += 1;
        }
        self.due
            .get(self.next)
            .map_or(frames, |due| (due.sample - now).min(frames as u64) as usize)
    }
}

#[cfg(test)]
mod end_tests;
#[cfg(test)]
mod hold_tests;
#[cfg(test)]
mod tests;
