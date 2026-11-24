//! Tells each module that measures its clock input (a sequencer) how far
//! apart that input's edges will be, when a clock's gate output feeds it.
//!
//! A sequencer learns its step length from the gap between two clock
//! edges, so on the first edge it has measured nothing and would size the
//! first step's gate from a guess, and on the first edge after a tempo
//! change it has measured the old tempo. The clock already knows its tempo:
//! the graph hands the expected gap over whenever its modules, routes or a
//! clock's tempo change, before the next sample plays, so that step's gate
//! is as long as every later step's at the same tempo.
//!
//! Only a sequencer whose clock input a clock's gate output feeds directly
//! is told; one fed through another module (a vca, a divider, a script)
//! still measures, so its first gate, and its first after a tempo change,
//! keep their guess.

use super::SignalGraph;
use crate::control_request::{flush_staged, take_automation};

impl SignalGraph {
    /// Hands every module with a measured clock input the step length of
    /// the clock gate output feeding it, if exactly one does. A module fed
    /// by anything else (a mix, a divider, nothing) keeps its own guess.
    /// Allocation-, free- and lock-free.
    pub(super) fn expect_clock_steps(&mut self) {
        for index in 0..self.modules.len() {
            let Some(samples) = self.expected_clock_step(index) else {
                continue;
            };
            if let Some((_, instance)) = self.modules.get_index_mut(index) {
                instance.module_mut().expect_clock_step(samples);
            }
        }
    }

    /// Whether any clock's tempo changed since the step lengths were last
    /// handed out. A tempo written to a clock is taken first, staged or
    /// automated, as it is before beats are placed on it, so a write due at
    /// this sample counts before the sequencers process it. One call per
    /// module that keeps a timeline; allocation-, free- and lock-free.
    pub(super) fn tempo_changed(&mut self) -> bool {
        self.with_drain(|graph, drain| {
            for (_, instance) in graph.modules.iter_mut() {
                if instance.module().timeline().is_some() {
                    if drain.pending.outcomes.staged() > 0 {
                        flush_staged(instance.module_mut(), &mut drain.pending.outcomes);
                    }
                    take_automation(instance.module_mut());
                }
            }
        });
        let changes = self
            .modules
            .values()
            .filter_map(|instance| instance.module().timeline())
            .fold(0u64, |sum, timeline| {
                sum.wrapping_add(timeline.tempo_changes())
            });
        changes != std::mem::replace(&mut self.tempo_changes_seen, changes)
    }

    /// How many samples apart the gates feeding module `index`'s measured
    /// clock input rise at the clock's current tempo.
    fn expected_clock_step(&self, index: usize) -> Option<u32> {
        let (id, instance) = self.modules.get_index(index)?;
        let input = instance.module().measured_clock_input()?;
        let mut sources = self
            .edges
            .iter()
            .filter(|edge| edge.to_module == *id && edge.to_port == input);
        let (Some(edge), None) = (sources.next(), sources.next()) else {
            return None;
        };
        let clock = self.modules.get(&edge.from_module)?.module();
        let (_, beats) = clock.gate_beats(&edge.from_port)?;
        let timeline = clock.timeline()?;
        let samples = timeline.samples_until(timeline.position() + beats)?;
        u32::try_from(samples).ok().filter(|&n| n > 0)
    }
}
