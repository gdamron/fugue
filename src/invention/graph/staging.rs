//! Plain writes staged until their module processes (see
//! [`crate::control_request::stage_write`]): where the graph stages a
//! request's write, and where it flushes what is staged.

use super::SignalGraph;
use crate::control_request::{
    flush_staged, stage_write, ControlTarget, Outcome, Outcomes, RequestValue, Standing,
};

impl SignalGraph {
    /// Applies a request as [`Self::apply_ranked`] does, staging a plain
    /// write of a rank for the request [`Outcomes::applying`] named: its
    /// setter runs as its module processes, or at a flush before. Anything
    /// else applies at once, after what is staged on its module. A write a
    /// condition waits on applies at once too, so the condition learns of
    /// it as it applies (see `Chains::watching`). Allocation-, free- and
    /// lock-free.
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
        self.flush_module(target.module_idx, outcomes);
        self.apply_ranked(target, standing, value, &mut outcomes.retirer)
    }

    /// Whether a write for request `id` may stage: a test's request hook
    /// applies every write as it comes.
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

    /// Flushes what is staged on every module: before anything reads them
    /// as a whole (a clock's timeline), or an edit replaces them.
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

    /// [`Self::flush_module`] through the graph's own drain, as the module
    /// is about to process; nothing when the drain is out (its holder
    /// flushes).
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
