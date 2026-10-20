//! Requests timed in beats on the audio thread: each waits on its clock
//! and applies at the sample its clock's position reaches the beat (see
//! [`crate::control_request::Watches`]). The modules' tables play there
//! too (see [`crate::control_request::BeatTable`]), each on the clock
//! output connected to its clock input.

use super::SignalGraph;
use crate::control_request::{
    kept_hold, take_automation, BeatHost, BeatTable, ControlTarget, Outcome, Outcomes,
    RequestValue, RtValue, Standing, StepClock, Timeline, Watches,
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

    fn current(&mut self, target: &ControlTarget) -> Option<f32> {
        let (_, instance) = self.modules.get_index_mut(target.module_idx)?;
        // Automation still waiting was written before this sample.
        take_automation(instance.module_mut());
        let module = instance.module();
        // A development's control reads its first alias, to the leaf, as a
        // scheduler's ramp does (`Automation::current`).
        let value = match module.alias_value(target.control) {
            Some(value) => value,
            None => module.declared()?.1.load(target.control)?,
        };
        match value {
            RtValue::F32(value) => Some(value),
            _ => None,
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

    fn table(&self, module_idx: usize) -> Option<&dyn BeatTable> {
        let (_, instance) = self.modules.get_index(module_idx)?;
        instance.module().beat_table()
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
    /// due then, then writes every ramp running its value there, and
    /// returns how many samples until the next watch may be due or ramp
    /// arrive: the segment then ends just before it. Allocation-, free-
    /// and lock-free.
    pub(super) fn watch_beats(
        &mut self,
        watches: &mut Watches,
        outcomes: &mut Outcomes,
        installed: u64,
    ) -> Option<u64> {
        if std::mem::take(&mut self.tables_stale) {
            self.find_tables(watches);
        }
        if watches.is_empty() {
            return None;
        }
        // A clock's position counts its automation from here, as its
        // requests do (written before this sample, taken before it plays).
        // So a request timed in beats applies after automation written for
        // the same sample, and wins; one timed in samples applies before
        // it, and the automation wins.
        self.take_clocks(watches, installed);
        // An application can move a clock (a reset or tempo timed on
        // beats), and so can a ramp's write (one of a tempo), so pass again
        // after them until none applies, the deadlines then as the clocks
        // run after every write: each watch applies at most once a sample,
        // and a ramp writes the same value again at the same sample, so
        // this ends.
        //
        // A table's entry, or its ramp, writes automation, which every
        // clock waited on then takes, so the deadlines see a tempo written.
        let now = self.current_sample;
        loop {
            if watches.pass(now, installed, self, outcomes).0 {
                self.take_clocks(watches, installed);
                continue;
            }
            watches.ramps.step(installed, self, &mut outcomes.retirer);
            self.take_clocks(watches, installed);
            let arrives = watches.ramps.arrival(installed, self);
            let (applied, wait) = watches.pass(now, installed, self, outcomes);
            self.take_clocks(watches, installed);
            if !applied {
                return match (wait, arrives) {
                    (Some(wait), Some(arrives)) => Some(wait.min(arrives)),
                    (wait, arrives) => wait.or(arrives),
                };
            }
        }
    }

    /// Takes the automation written to every clock a watch, ramp or table
    /// waits on.
    fn take_clocks(&mut self, watches: &Watches, installed: u64) {
        for clock in watches.clocks(installed) {
            if let Some((_, instance)) = self.modules.get_index_mut(clock) {
                take_automation(instance.module_mut());
            }
        }
    }

    /// Finds every module's table and the clock it steps on, after the
    /// modules or their routes changed, telling each table whether the
    /// drain plays it: a table finding no clock output on its clock input
    /// (nothing, a mix of sources, or another signal) or no room plays on
    /// its own. The tables played before are found first, so the room of
    /// those gone is free for new ones. Allocation-, free- and lock-free.
    fn find_tables(&self, watches: &mut Watches) {
        let Watches { cursors, ramps, .. } = watches;
        cursors.seek();
        for known in [true, false] {
            for (module, (id, instance)) in self.modules.iter().enumerate() {
                let Some(table) = instance.module().beat_table() else {
                    continue;
                };
                if cursors.has(table.id()) != known {
                    continue;
                }
                let input = table.clock_input();
                let mut sources = self
                    .edges
                    .iter()
                    .filter(|edge| edge.to_module == *id && edge.to_port == input);
                let clock = match (sources.next(), sources.next()) {
                    (Some(edge), None) => self.step_clock(&edge.from_module, &edge.from_port),
                    _ => None,
                };
                table.set_played(cursors.found(table.id(), module, clock, ramps));
            }
            if known {
                cursors.sought(ramps);
            }
        }
    }

    /// The clock whose output `port` of module `id` is, if it is one, told
    /// apart by its instance's token: the same wherever it moves, another
    /// for a clock replacing it.
    fn step_clock(&self, id: &str, port: &str) -> Option<StepClock> {
        let (module, _, instance) = self.modules.get_full(id)?;
        let (id, beats) = instance.module().gate_beats(port)?;
        Some(StepClock { module, id, beats })
    }
}
