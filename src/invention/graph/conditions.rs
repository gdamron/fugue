//! Requests timed by a condition on the audio thread: what the graph lets
//! the table read, and where in a segment it applies them (see
//! [`crate::control_request::Conditions`]).
//!
//! A segment's groups run in order. Before a target module computes, the
//! table reads what every module it watches has computed of the segment
//! (a module in an earlier group: all of it) and applies each request due
//! by then; when one comes due inside the segment, the target computes up
//! to that sample, takes it, and computes on. So a condition on modules
//! ordered before its target applies at its exact sample, and nothing else
//! changes: modules not targeted compute their segment whole.
//!
//! What the target cannot see before it computes (a module after it in the
//! order, its own output, one in its feedback group) is read at the end of
//! the segment, and the request applies before the target's next sample,
//! reported late. A target in a feedback group, or a sink, is not split:
//! what comes due inside its segment applies there too.

use super::requests::RequestDrain;
use super::SignalGraph;
use crate::control_request::{
    ConditionHost, ControlIndex, ControlTarget, Outcome, Ramps, RequestValue, RtValue, Standing,
};
use crate::payload::Retirer;

/// The segment being processed: its first sample, its length, and how many
/// process groups have computed it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Segment {
    pub(super) start: u64,
    pub(super) frames: usize,
    pub(super) computed: usize,
}

impl ConditionHost for SignalGraph {
    fn output(&self, module: usize, port: usize) -> Option<(u64, &[f32])> {
        if port >= *self.out_counts.get(module)? {
            return None;
        }
        let Segment {
            start,
            frames,
            computed,
        } = self.segment;
        let frames = if self.group_of[module] < computed {
            frames
        } else {
            0
        };
        let base = port * self.block_capacity;
        Some((start, &self.out_bufs[module][base..base + frames]))
    }

    fn last_output(&self, module: usize, port: usize) -> Option<f32> {
        self.out_prev.get(module)?.get(port).copied()
    }

    fn control(&self, module: usize, control: ControlIndex) -> Option<f32> {
        let (_, instance) = self.modules.get_index(module)?;
        let (_, cells) = instance.module().declared()?;
        Some(match cells.load(control)? {
            RtValue::F32(value) => value,
            RtValue::U32(value) => value as f32,
            RtValue::I32(value) => value as f32,
            RtValue::Bool(value) => f32::from(u8::from(value)),
        })
    }

    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome> {
        self.apply_ranked(target, standing, value, retirer)
    }
}

/// The graph as a condition's requests apply to it: each write ends every
/// ramp in beats on a control it reaches, as any other write does (see
/// [`Ramps::supersede`]). One at a ramp's own rank leaves the hold as the
/// ramp took it, so the hold alone would not end it.
struct Superseding<'a> {
    graph: &'a mut SignalGraph,
    ramps: &'a mut Ramps,
}

impl ConditionHost for Superseding<'_> {
    fn output(&self, module: usize, port: usize) -> Option<(u64, &[f32])> {
        ConditionHost::output(&*self.graph, module, port)
    }

    fn last_output(&self, module: usize, port: usize) -> Option<f32> {
        ConditionHost::last_output(&*self.graph, module, port)
    }

    fn control(&self, module: usize, control: ControlIndex) -> Option<f32> {
        ConditionHost::control(&*self.graph, module, control)
    }

    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome> {
        let applied = self.graph.apply_ranked(target, standing, value, retirer);
        if applied.is_ok() {
            self.ramps.supersede(target, &*self.graph);
        }
        applied
    }
}

impl SignalGraph {
    /// Whether a request timed by a condition waits: only then does a
    /// segment look for one.
    pub(super) fn conditions_waiting(&self) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|drain| !drain.conditions.is_empty())
    }

    /// Processes the acyclic module `module` over the current segment's
    /// `frames`, applying each request on it whose condition is met at or
    /// before a sample before it computes that sample. Allocation-, free-
    /// and lock-free.
    pub(super) fn process_conditioned(&mut self, module: usize, frames: usize) {
        let Some(mut drain) = self.requests.take() else {
            return self.process_span(module, 0, frames);
        };
        let chains = &drain.pending.outcomes.chains;
        drain
            .conditions
            .read(drain.installed, Some(module), self, chains);
        let start = self.segment.start;
        let mut from = 0;
        while from < frames {
            let at = start + from as u64;
            let next = self.apply_conditions(&mut drain, Some(module), at);
            let to = next.map_or(frames, |due| ((due - start) as usize).min(frames));
            self.process_span(module, from, to);
            from = to;
        }
        self.requests = Some(drain);
    }

    /// Applies what is due on `module` as its group starts, for a module
    /// that is not split (a sink, a feedback group's member).
    pub(super) fn apply_conditions_on(&mut self, module: usize) {
        let Some(mut drain) = self.requests.take() else {
            return;
        };
        let chains = &drain.pending.outcomes.chains;
        drain
            .conditions
            .read(drain.installed, Some(module), self, chains);
        self.apply_conditions(&mut drain, Some(module), self.segment.start);
        self.requests = Some(drain);
    }

    /// Applies every request in `drain` (on `target`, if given) whose
    /// condition is met by `at`, the sample its target computes next, and
    /// those met by their applying in turn: a chain applies in one pass.
    /// Returns the earliest sample one of the rest is met at.
    pub(super) fn apply_conditions(
        &mut self,
        drain: &mut RequestDrain,
        target: Option<usize>,
        at: u64,
    ) -> Option<u64> {
        let installed = drain.installed;
        loop {
            let waiting = drain.conditions.len();
            let outcomes = &mut drain.pending.outcomes;
            let mut host = Superseding {
                graph: self,
                ramps: &mut drain.watches.ramps,
            };
            let next = drain
                .conditions
                .apply_due(installed, target, at, &mut host, outcomes);
            if drain.conditions.len() == waiting {
                return next;
            }
            drain.conditions.chain(installed, &outcomes.chains);
        }
    }

    /// Reads what every module computed of the segment just processed, for
    /// the conditions their targets could not see in time.
    pub(super) fn read_conditions(&mut self) {
        if let Some(mut drain) = self.requests.take() {
            let chains = &drain.pending.outcomes.chains;
            drain.conditions.read(drain.installed, None, self, chains);
            self.requests = Some(drain);
        }
    }
}
