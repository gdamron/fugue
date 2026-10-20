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

use super::SignalGraph;
use crate::control_request::{ConditionHost, ControlTarget, Outcome, RequestValue, Standing};
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
        let installed = drain.installed;
        drain.conditions.read(installed, Some(module), self);
        let start = self.segment.start;
        let mut from = 0;
        while from < frames {
            let at = start + from as u64;
            let outcomes = &mut drain.pending.outcomes;
            let next = drain
                .conditions
                .apply_due(installed, Some(module), at, self, outcomes);
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
        let installed = drain.installed;
        drain.conditions.read(installed, Some(module), self);
        let outcomes = &mut drain.pending.outcomes;
        let at = self.segment.start;
        drain
            .conditions
            .apply_due(installed, Some(module), at, self, outcomes);
        self.requests = Some(drain);
    }

    /// Reads what every module computed of the segment just processed, for
    /// the conditions their targets could not see in time.
    pub(super) fn read_conditions(&mut self) {
        if let Some(mut drain) = self.requests.take() {
            drain.conditions.read(drain.installed, None, self);
            self.requests = Some(drain);
        }
    }
}
