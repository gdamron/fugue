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

use super::publication::Publication;
use super::requests::RequestDrain;
use super::SignalGraph;
use crate::control_request::{
    ConditionHost, ControlIndex, ControlTarget, Outcome, Outcomes, Ramps, RequestValue, RtValue,
    Standing, Watches,
};

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
        // A write staged on it is what it holds as it processes.
        Some(match cells.current(control)? {
            RtValue::F32(value) => value,
            RtValue::U32(value) => value as f32,
            RtValue::I32(value) => value as f32,
            RtValue::Bool(value) => f32::from(u8::from(value)),
        })
    }

    fn edit_cancelled(&self, value: &RequestValue) -> bool {
        let RequestValue::Edit(payload) = value else {
            return false;
        };
        let publication = payload.peek::<Publication>();
        let fate = publication.and_then(|publication| publication.fate.as_ref());
        fate.is_some_and(|fate| fate.cancelled())
    }

    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        outcomes: &mut Outcomes,
    ) -> Result<(), Outcome> {
        self.stage(target, standing, value, outcomes)
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

    fn edit_cancelled(&self, value: &RequestValue) -> bool {
        self.graph.edit_cancelled(value)
    }

    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        outcomes: &mut Outcomes,
    ) -> Result<(), Outcome> {
        let applied = self.graph.stage(target, standing, value, outcomes);
        if applied.is_ok() {
            self.ramps.supersede(target, &*self.graph);
        }
        applied
    }

    fn finish_events(&mut self) {
        self.graph.finish_events();
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
        let spans = |graph: &mut Self, drain: &mut RequestDrain| {
            graph.process_conditioned_spans(drain, module, frames)
        };
        if self.with_drain(spans).is_none() {
            self.process_span(module, 0, frames);
        }
    }

    /// [`Self::process_conditioned`] with the drain taken out of the graph.
    fn process_conditioned_spans(
        &mut self,
        drain: &mut RequestDrain,
        module: usize,
        frames: usize,
    ) {
        let chains = &drain.pending.outcomes.chains;
        drain
            .conditions
            .read(drain.installed, Some(module), self, chains);
        // So do the entries of tables played on edges (see `super::edges`),
        // after the requests due at the same sample.
        let edges = !drain.watches.edges.is_empty();
        if edges {
            drain.watches.edges.begin(module, self);
        }
        let start = self.segment.start;
        let mut from = 0;
        while from < frames {
            let at = start + from as u64;
            let next = self.apply_conditions(drain, Some(module), at);
            let mut to = next.map_or(frames, |due| ((due - start) as usize).min(frames));
            if edges {
                let Watches { edges, ramps, .. } = &mut drain.watches;
                to = to.min(edges.play(module, from, self, ramps));
            }
            // The drain is out: `process_span` cannot flush.
            self.flush_module(module, &mut drain.pending.outcomes);
            self.process_span(module, from, to);
            from = to;
        }
    }

    /// Applies what is due on `module` as its group starts, for a module
    /// that is not split (a sink, a feedback group's member).
    pub(super) fn apply_conditions_on(&mut self, module: usize) {
        self.with_drain(|graph, drain| {
            let chains = &drain.pending.outcomes.chains;
            drain
                .conditions
                .read(drain.installed, Some(module), graph, chains);
            graph.apply_conditions(drain, Some(module), graph.segment.start);
        });
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
            let mut host = Superseding {
                graph: self,
                ramps: &mut drain.watches.ramps,
            };
            let pending = &mut drain.pending;
            let next = drain
                .conditions
                .apply_due(installed, target, at, &mut host, pending);
            if drain.conditions.len() == waiting {
                return next;
            }
            drain.conditions.chain(installed, &pending.outcomes.chains);
        }
    }

    /// Reads what every module computed of the segment just processed, for
    /// the conditions their targets could not see in time.
    pub(super) fn read_conditions(&mut self) {
        self.with_drain(|graph, drain| {
            let chains = &drain.pending.outcomes.chains;
            drain.conditions.read(drain.installed, None, graph, chains);
        });
    }
}
