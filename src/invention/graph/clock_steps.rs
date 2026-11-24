//! Tells each module that measures its clock input (a sequencer) how far
//! apart that input's edges will be, when a clock's gate output feeds it.
//!
//! A sequencer learns its step length from the gap between two clock
//! edges, so on the first edge it has measured nothing and would size the
//! first step's gate from a guess. The clock already knows its tempo: the
//! graph hands the expected gap over whenever its modules or routes change,
//! before the next block plays, so the first gate is as long as every
//! later pass of the same step.

use super::SignalGraph;

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
