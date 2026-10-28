//! Where the graph stages a request's write, and flushes what is staged
//! (see [`crate::control_request::stage_write`]).

use super::SignalGraph;
use crate::control_request::{
    flush_staged, stage_write, ControlTarget, Outcome, Outcomes, Ramps, RequestValue, Standing,
};

impl SignalGraph {
    /// [`Self::apply_ranked`], staging a plain write: anything else, or a
    /// write a condition waits on (`Chains::watching`), applies at once.
    pub(super) fn stage(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        outcomes: &mut Outcomes,
    ) -> Result<(), Outcome> {
        let applying = outcomes.take_applying();
        if let (Some(applying), Some(standing), RequestValue::Value(write)) =
            (applying, standing, &value)
        {
            let stages = self.stages(applying.id, outcomes);
            let instance = self.modules.get_index_mut(target.module_idx);
            if let (true, Some((_, instance))) = (stages, instance) {
                let module = instance.module_mut();
                if stage_write(module, target.control, *write, standing, applying, outcomes)? {
                    return Ok(());
                }
            }
        }
        self.apply_now(target, standing, value, outcomes)
    }

    /// [`Self::stage`], ending the request ramps on `target` once it
    /// applies. A write to a ramping control applies at once, so one its
    /// module refuses leaves the ramp running.
    pub(super) fn stage_over_ramps(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        outcomes: &mut Outcomes,
        ramps: &mut Ramps,
    ) -> Result<(), Outcome> {
        let applied = match ramps.ramping(target, &*self) {
            true => self.apply_now(target, standing, value, outcomes),
            false => self.stage(target, standing, value, outcomes),
        };
        if applied.is_ok() {
            ramps.supersede(target, &*self);
        }
        applied
    }

    /// Applies a request at once, after what is staged on its module.
    pub(super) fn apply_now(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        outcomes: &mut Outcomes,
    ) -> Result<(), Outcome> {
        outcomes.take_applying();
        self.flush_module(target.module_idx, outcomes);
        self.apply_ranked(target, standing, value, &mut outcomes.retirer)
    }

    /// Whether `id`'s write may stage (never under a test's request hook).
    fn stages(&self, id: crate::control_request::RequestId, outcomes: &Outcomes) -> bool {
        #[cfg(test)]
        if self.request_hook.is_some() {
            return false;
        }
        !outcomes.chains.watching(id)
    }

    /// Flushes what is staged on the module at `module_idx`.
    pub(super) fn flush_module(&mut self, module_idx: usize, outcomes: &mut Outcomes) {
        if let Some((_, instance)) = self.modules.get_index_mut(module_idx) {
            flush_staged(instance.module_mut(), outcomes);
        }
    }

    /// Flushes every module's staged writes.
    pub(super) fn flush_all(&mut self, outcomes: &mut Outcomes) {
        if outcomes.staged() == 0 {
            return;
        }
        for (_, instance) in self.modules.iter_mut() {
            flush_staged(instance.module_mut(), outcomes);
        }
    }

    /// [`Self::flush_all`] through the graph's own drain.
    pub(super) fn flush_all_staged(&mut self) {
        self.with_drain(|graph, drain| graph.flush_all(&mut drain.pending.outcomes));
    }

    /// [`Self::flush_module`] through the graph's drain, when it holds it.
    pub(super) fn flush_staged_on(&mut self, module_idx: usize) {
        let (Some(drain), Some((_, instance))) = (
            self.requests.as_mut(),
            self.modules.get_index_mut(module_idx),
        ) else {
            return;
        };
        flush_staged(instance.module_mut(), &mut drain.pending.outcomes);
    }
}
