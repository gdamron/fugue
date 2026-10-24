//! Replaying a live session's request log in an offline render.
//!
//! Each logged request applies at its recorded sample, by splitting the
//! render there, exactly as the live drain split the block it applied in,
//! and through the same apply a live request takes (automation still
//! waiting first, then the module's declared control). A request that
//! applied on its clock's beat latches the clock first, as the live watch
//! did, so a tempo it changes there takes effect from the same sample. A
//! watch refused or overridden on its beat, a timed edit's among them,
//! latched its clock live too: the log records each such latch, and the
//! replay splits the render there and latches the clock after the requests
//! logged before it, so a tempo written to that clock later in the sample
//! takes effect from the next one, as it did live.
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
//!
//! A timed edit that landed lands on the same sample, after every request
//! logged at or before it, as the live landing came after every request due
//! at its segment start; the render makes it through the offline edit path
//! (see `editing`), as a timed edit lands live: what it displaces plays
//! first, and one taken on its clock's beat latches the clock where live
//! did, before the requests after it. One refused live is left out, but
//! for the latch it made if it was refused on its beat. Every latch at a
//! sample comes before an edit landing there, as the live watches pass
//! before the landing. Requests after an edit
//! resolve against the graph it left: one naming a control not declared
//! there is counted refused.

use indexmap::IndexMap;

use crate::control_request::{take_automation, ControlIndex, RtValue};
use crate::invention::graph::SignalGraph;
use crate::invention::runtime::ControlSurfaceInstance;
use crate::{ModuleRegistry, RequestLog};

mod edits;
mod latches;

pub(super) use edits::DueEdit;

/// A logged request resolved against the render's surfaces.
struct Due {
    sample: u64,
    /// The sample it was due at, which its hold counts from.
    due: u64,
    module_id: String,
    key: String,
    /// Resolved against the graph as it will be when the request is due:
    /// as loaded, or as the last edit before it left it.
    control: Option<ControlIndex>,
    value: RtValue,
    /// The clock it applied on the beat of, which it latches first.
    latch: Option<String>,
}

/// What a render does next while it replays.
pub(super) enum Next {
    /// Render this many frames.
    Render(usize),
    /// Land the next timed edit (see [`Replay::take_edit`]) first.
    Edit,
}

/// The requests and timed edits a render still has to apply, in log order.
#[derive(Default)]
pub(super) struct Replay {
    due: Vec<Due>,
    next: usize,
    /// Requests resolved so far: up to the next edit's sample.
    resolved: usize,
    edits: Vec<DueEdit>,
    next_edit: usize,
    /// The clock latches apart from the requests' (see [`latches`]).
    latches: Vec<latches::DueLatch>,
    next_latch: usize,
    /// Requests the render's graph refused (it would not have logged them).
    pub(super) refused: usize,
}

impl Replay {
    /// Resolves `log` against the render: every request up to its first
    /// landed timed edit to a declared control of a module in `surfaces`
    /// (later ones resolve as the edits before them land), and every
    /// module an edit builds, built now from `registry`. Fails naming the
    /// first request that names no declared control, the first request or
    /// edit out of time order, or the first module that does not build.
    pub(super) fn resolve(
        log: &RequestLog,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
        (registry, sample_rate): (&ModuleRegistry, u32),
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
                // Live latches a clock for a watch applied on its beat, not
                // for one that applied late.
                let latch = match (&request.beat, request.due) {
                    (Some(beat), None) => Some(beat.clock().to_string()),
                    _ => None,
                };
                Due {
                    sample: request.sample,
                    due: request.due.unwrap_or(request.sample).min(request.sample),
                    module_id: request.module_id.clone(),
                    key: request.key.clone(),
                    control: None,
                    value: request.value.into(),
                    latch,
                }
            })
            .collect();
        let mut replay = Self {
            due,
            edits: edits::build(log, registry, sample_rate)?,
            latches: latches::build(log),
            ..Self::default()
        };
        let through = replay.edits.first().map(|edit| edit.sample);
        if let Some(missing) = replay.resolve_through(surfaces, through) {
            let request = &replay.due[missing];
            return Err(format!(
                "the logged request for '{}' of '{}' names no declared control",
                request.key, request.module_id
            ));
        }
        Ok(replay)
    }

    /// Resolves the requests not yet resolved, up to sample `through`
    /// (all, for none), against `surfaces`. Returns the first that names
    /// no declared control there, which stays unresolved.
    fn resolve_through(
        &mut self,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
        through: Option<u64>,
    ) -> Option<usize> {
        let mut missing = None;
        while let Some(due) = self.due.get_mut(self.resolved) {
            if through.is_some_and(|through| due.sample > through) {
                break;
            }
            due.control = surfaces
                .get(&due.module_id)
                .and_then(|surface| surface.declaration(&due.key))
                .map(|declaration| declaration.index);
            if due.control.is_none() {
                missing.get_or_insert(self.resolved);
            }
            self.resolved += 1;
        }
        missing
    }

    /// Makes each latch due by `now` once the requests applied before it
    /// live have applied.
    fn latch_due(&mut self, graph: &mut SignalGraph, now: u64) {
        while let Some(due) = self.latches.get(self.next_latch) {
            if due.sample > now || due.requests_before > self.next {
                return;
            }
            latch(graph, &due.clock);
            self.next_latch += 1;
        }
    }

    /// The next timed edit, now due, to land; afterwards the requests up to
    /// the one after it resolve against `surfaces` as it left them.
    pub(super) fn take_edit(&mut self) -> Option<DueEdit> {
        let edit = self.edits.get_mut(self.next_edit)?;
        self.next_edit += 1;
        Some(std::mem::take(edit))
    }

    /// Resolves the requests due before the next timed edit against
    /// `surfaces`, as the edit just landed left them.
    pub(super) fn landed(&mut self, surfaces: &IndexMap<String, ControlSurfaceInstance>) {
        let through = self.edits.get(self.next_edit).map(|edit| edit.sample);
        self.resolve_through(surfaces, through);
    }

    /// Fails naming the first clock a logged beat names that keeps no beat
    /// timeline in `graph`: its latch would silently do nothing.
    /// Only those resolved yet, and the latches made before the first
    /// timed edit lands: a later one may name a clock an edit adds.
    pub(super) fn check_clocks(&self, graph: &mut SignalGraph) -> Result<(), String> {
        let resolved = &self.due[..self.resolved];
        let first = self.edits.first().map(|edit| edit.sample);
        let before = |due: &&latches::DueLatch| first.is_none_or(|first| due.sample <= first);
        let latched = self.latches.iter().take_while(before).map(|due| &due.clock);
        let clocks = resolved.iter().filter_map(|due| due.latch.as_ref());
        for clock in clocks.chain(latched) {
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
    /// past applies now, as a late request would), and returns what to do
    /// next: land a timed edit due now, or render up to `frames` frames,
    /// until the next request or edit is due.
    pub(super) fn apply_due(&mut self, graph: &mut SignalGraph, frames: usize) -> Next {
        let now = graph.current_sample;
        loop {
            self.latch_due(graph, now);
            let Some(due) = self.due.get(self.next).filter(|due| due.sample <= now) else {
                break;
            };
            if let Some(clock) = &due.latch {
                latch(graph, clock);
            }
            let applied = due.control.map(|control| {
                graph.apply_control_due(&due.module_id, control, due.value, due.due)
            });
            if !matches!(applied, Some(Ok(_))) {
                self.refused += 1;
            }
            self.next += 1;
        }
        let edit = self.edits.get(self.next_edit).map(|edit| edit.sample);
        if edit.is_some_and(|sample| sample <= now) {
            return Next::Edit;
        }
        let request = self.due.get(self.next).map(|due| due.sample);
        // A latch splits the render where live split it, on its beat.
        let latch = (self.latches.get(self.next_latch)).map(|due| due.sample);
        let latch = latch.filter(|sample| *sample > now);
        let until = [request, edit, latch].into_iter().flatten().min();
        Next::Render(until.map_or(frames, |at| (at - now).min(frames as u64) as usize))
    }
}

/// Latches `clock` as the live watch does: automation written before this
/// sample first, then the latch.
fn latch(graph: &mut SignalGraph, clock: &str) {
    if let Some(clock) = graph.modules.get_mut(clock) {
        take_automation(clock.module_mut());
        if let Some(timeline) = clock.module_mut().timeline_mut() {
            timeline.latch();
        }
    }
}

#[cfg(test)]
mod development_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod hold_tests;
#[cfg(test)]
mod landing_tests;
#[cfg(test)]
mod latch_tests;
#[cfg(test)]
mod tests;
