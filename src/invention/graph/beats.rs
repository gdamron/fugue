//! Requests timed in beats on the audio thread: each waits on its clock
//! and applies at the sample its clock's position reaches the beat (see
//! [`crate::control_request::Watches`]).

use super::SignalGraph;
use crate::control_request::{
    kept_hold, take_automation, BeatHost, ControlTarget, Outcome, Outcomes, RequestValue, Standing,
    Timeline, Watches,
};
use crate::payload::Retirer;

impl BeatHost for SignalGraph {
    fn timeline(&self, module_idx: usize) -> Option<&dyn Timeline> {
        let (_, instance) = self.modules.get_index(module_idx)?;
        instance.module().timeline()
    }

    fn latch(&mut self, module_idx: usize) {
        if let Some((_, instance)) = self.modules.get_index_mut(module_idx) {
            if let Some(timeline) = instance.module_mut().timeline_mut() {
                timeline.latch();
            }
        }
    }

    fn kept(&self, target: &ControlTarget, standing: Standing) -> bool {
        self.modules
            .get_index(target.module_idx)
            .is_some_and(|(_, instance)| kept_hold(instance.module(), target.control, standing))
    }

    fn overlaps(&self, a: &ControlTarget, b: &ControlTarget) -> bool {
        // Two modules' controls are apart: a development's inner graph is
        // its own.
        if a.module_idx != b.module_idx {
            return false;
        }
        if a.control == b.control {
            return true;
        }
        let Some((_, instance)) = self.modules.get_index(a.module_idx) else {
            return false;
        };
        let module = instance.module();
        let mut shared = false;
        module.for_each_alias(a.control, &mut |_, cells, index| {
            module.for_each_alias(b.control, &mut |_, other, other_index| {
                shared |= std::ptr::eq(cells, other) && index == other_index;
            });
        });
        shared
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
    /// Applies every watch due at the current sample, after the requests
    /// due then, and returns how many samples until the next may be: the
    /// segment then ends just before it. Allocation-, free- and lock-free.
    pub(super) fn watch_beats(
        &mut self,
        watches: &mut Watches,
        outcomes: &mut Outcomes,
        installed: u64,
    ) -> Option<u64> {
        if watches.is_empty() {
            return None;
        }
        // A clock's position counts its automation from here, as its
        // requests do (written before this sample, taken before it plays).
        // So a request timed in beats applies after automation written for
        // the same sample, and wins; one timed in samples applies before
        // it, and the automation wins.
        for clock in watches.clocks(installed) {
            if let Some((_, instance)) = self.modules.get_index_mut(clock) {
                take_automation(instance.module_mut());
            }
        }
        // An application can move a clock (a reset or tempo timed on
        // beats), so pass again until none applies: each applies at most
        // once a sample, so this ends.
        let now = self.current_sample;
        loop {
            let (applied, wait) = watches.pass(now, installed, self, outcomes);
            if !applied {
                return wait;
            }
        }
    }
}
