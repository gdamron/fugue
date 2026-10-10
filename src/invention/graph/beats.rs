//! Requests timed in beats on the audio thread: each waits on its clock
//! and applies at the sample its clock's position reaches the beat (see
//! [`crate::control_request::Watches`]).

use super::SignalGraph;
use crate::control_request::{
    take_automation, BeatHost, ControlTarget, Outcomes, Refusal, RequestValue, Timeline, Watches,
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

    fn apply(
        &mut self,
        target: &ControlTarget,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Refusal> {
        self.apply_request(target.module_idx, target.control, value, retirer)
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
