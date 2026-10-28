//! Where the graph stages a request's write, and flushes what is staged
//! (see [`crate::control_request::stage_write`]).

use super::{Publication, SignalGraph};
use crate::control_request::{
    contest, flush_staged, flush_staged_before, flush_watched, stage_write, stand, Applying,
    Before, ControlTarget, Outcome, Outcomes, Ramps, RequestValue, Standing,
};

impl SignalGraph {
    /// [`Self::apply_ranked`], staging a plain write: anything else, or a
    /// write a condition waits on (`Chains::watching`), applies at once.
    /// Either way, the one write standing on the control at the sample
    /// applies ([`contest`]).
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
        self.apply_at_once(target, standing, value, applying, outcomes)
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
        let applying = outcomes.take_applying();
        self.apply_at_once(target, standing, value, applying, outcomes)
    }

    /// [`Self::apply_now`] for the request `applying` names, if any: a
    /// value or payload write weighed against the one standing on its
    /// control at the sample, after the weaker writes staged on its module
    /// and before the stronger, as if each applied on time; an event (no
    /// standing) after those due no later than it, before those due after.
    fn apply_at_once(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        applying: Option<Applying>,
        outcomes: &mut Outcomes,
    ) -> Result<(), Outcome> {
        let weighed = applying
            .zip(standing)
            .filter(|_| !matches!(value, RequestValue::Edit(_)));
        let index = target.module_idx;
        if let Some((_, instance)) = self.modules.get_index_mut(index) {
            let event =
                applying.filter(|_| standing.is_none() && matches!(value, RequestValue::Value(_)));
            let than = (weighed.map(|(applying, standing)| Before::write(standing, applying)))
                .or(event.map(Before::event));
            flush_staged_before(instance.module_mut(), than, outcomes);
            // Only the writes at its sample it runs before wait for it.
            let at = than.map(Before::at);
            debug_assert!(
                (instance.module().declared())
                    .is_none_or(|(_, cells)| cells.staged.waits_only_at(at)),
                "applied past a write staged before it"
            );
        }
        if let (Some((applying, standing)), Some((_, instance))) =
            (weighed, self.modules.get_index(index))
        {
            let module = instance.module();
            if let Err(lost) = contest(module, target.control, standing, applying, outcomes) {
                if let RequestValue::Payload(payload) = value {
                    outcomes.retirer.retire(payload);
                }
                return Err(lost);
            }
        }
        self.apply_ranked(target, standing, value, &mut outcomes.retirer)?;
        if let (Some((applying, standing)), Some((_, instance))) =
            (weighed, self.modules.get_index(index))
        {
            stand(instance.module(), target.control, standing, applying);
        }
        Ok(())
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

    /// Flushes what is staged on each running module `publication` does
    /// not keep, as it installs: those writes apply to the instance they
    /// reached, before it is replaced (and inherited from) or removed. A
    /// survivor keeps its staged writes, so the requests queued behind the
    /// edit are weighed against them at this sample ([`contest`]), as with
    /// no edit between them.
    pub(super) fn flush_replaced(&mut self, publication: &Publication) {
        self.with_drain(|graph, drain| {
            let outcomes = &mut drain.pending.outcomes;
            if outcomes.staged() == 0 {
                return;
            }
            for (id, instance) in graph.modules.iter_mut() {
                let at = publication.modules.get_index_of(id.as_str());
                if at.and_then(|at| publication.survivor.get(at)) != Some(&true) {
                    flush_staged(instance.module_mut(), outcomes);
                }
            }
        });
    }

    /// Flushes each module a condition waits on a staged write of, as
    /// conditions start: the write it waits on applied at its sample.
    pub(super) fn flush_watched(&mut self, outcomes: &mut Outcomes) {
        if outcomes.staged() == 0 {
            return;
        }
        for (_, instance) in self.modules.iter_mut() {
            flush_watched(instance.module_mut(), outcomes);
        }
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
