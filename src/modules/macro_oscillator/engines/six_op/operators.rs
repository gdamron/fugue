//! Six sine operators phase-modulating one another along an algorithm.
//!
//! The rendering follows Mutable Instruments' `plaits/dsp/fm/operator.h`
//! (MIT, Copyright 2021 Emilie Gillet): each operator's level moves in a
//! straight line across the block, and the feedback loop feeds back the
//! average of the looped operator's last two outputs, which keeps strong
//! feedback from squealing. Fugue renders every algorithm with one loop
//! over a modulation matrix rather than Plaits' per-algorithm calls. See
//! THIRD_PARTY_NOTICES.md.

use super::algorithms::{Algorithm, OPERATORS};
use crate::dsp::sine;

/// The highest phase step, in cycles per sample (the Nyquist frequency).
const MAX_STEP: f32 = 0.5;

/// The phase modulation full feedback applies per unit of output, in
/// cycles: half a cycle (π radians), the edge of noise.
pub(in crate::modules::macro_oscillator) const MAX_FEEDBACK: f32 = 0.5;

/// One voice's operators: their phases, current levels and the feedback
/// loop's memory.
#[derive(Clone, Debug)]
pub(in crate::modules::macro_oscillator) struct Operators {
    /// Phases in cycles, 0 to 1.
    phase: [f32; OPERATORS],
    /// Each operator's output level where the last block ended. A carrier's
    /// is its loudness; a modulator's is its modulation depth, in cycles.
    level: [f32; OPERATORS],
    /// The looped operator's last two outputs.
    feedback: [f32; 2],
}

impl Operators {
    pub fn new() -> Self {
        Self {
            phase: [0.0; OPERATORS],
            level: [0.0; OPERATORS],
            feedback: [0.0; 2],
        }
    }

    /// Starts every operator at phase 0, for a note that starts from
    /// silence and should sound the same each time.
    pub fn reset_phases(&mut self) {
        self.phase = [0.0; OPERATORS];
        self.feedback = [0.0; 2];
    }

    /// Adds one block of `algorithm` to `out`. `step` is each operator's
    /// phase step in cycles per sample, `level` the level each reaches by
    /// the block's end, and `feedback` the loop's depth, 0 to 1.
    pub fn render(
        &mut self,
        algorithm: &Algorithm,
        step: &[f32; OPERATORS],
        level: &[f32; OPERATORS],
        feedback: f32,
        out: &mut [f32],
    ) {
        let size = out.len();
        if size == 0 {
            return;
        }
        let scale = 1.0 / size as f32;
        let mut amplitude = self.level;
        let mut increment = [0.0; OPERATORS];
        let mut steps = [0.0; OPERATORS];
        let mut heard = [0.0; OPERATORS];
        // `inputs[k][j]` is 1 when operator `j` modulates operator `k`.
        let mut inputs = [[0.0f32; OPERATORS]; OPERATORS];
        for k in 0..OPERATORS {
            increment[k] = (level[k] - amplitude[k]) * scale;
            steps[k] = step[k].clamp(0.0, MAX_STEP);
            heard[k] = if algorithm.is_carrier(k) { 1.0 } else { 0.0 };
            for (j, input) in inputs[k].iter_mut().enumerate() {
                *input = f32::from(algorithm.modulators[k] >> j & 1);
            }
        }
        let (from, to) = (algorithm.feedback_from, algorithm.feedback_to);
        // Half the sum of the last two outputs is their average.
        let feedback = 0.5 * feedback.clamp(0.0, 1.0) * MAX_FEEDBACK;
        let [mut previous_0, mut previous_1] = self.feedback;
        let mut phase = self.phase;

        for sample in out.iter_mut() {
            let mut output = [0.0f32; OPERATORS];
            let mut sum = 0.0;
            for k in (0..OPERATORS).rev() {
                let mut modulation = 0.0;
                for j in (k + 1)..OPERATORS {
                    modulation += inputs[k][j] * output[j];
                }
                if k == to {
                    modulation += feedback * (previous_0 + previous_1);
                }
                phase[k] += steps[k];
                phase[k] -= phase[k].floor();
                let y = sine(phase[k] + modulation) * amplitude[k];
                amplitude[k] += increment[k];
                if k == from {
                    previous_1 = previous_0;
                    previous_0 = y;
                }
                output[k] = y;
                sum += heard[k] * y;
            }
            *sample += sum;
        }

        self.phase = phase;
        // Land exactly on the targets, free of rounding drift.
        self.level = *level;
        self.feedback = [previous_0, previous_1];
    }
}

#[cfg(test)]
mod tests {
    use super::super::algorithms::ALGORITHMS;
    use super::*;

    /// A block of `n` frames of `algorithm` with the given levels.
    fn play(algorithm: usize, level: [f32; 6], feedback: f32, n: usize) -> Vec<f32> {
        let mut ops = Operators::new();
        let step = [440.0 / 48_000.0; 6];
        let mut out = vec![0.0; n];
        for block in out.chunks_mut(12) {
            ops.render(&ALGORITHMS[algorithm], &step, &level, feedback, block);
        }
        out
    }

    #[test]
    fn a_lone_carrier_is_a_sine_at_its_level() {
        // Algorithm 32 has six carriers; only operator 1 is raised.
        let out = play(31, [0.5, 0.0, 0.0, 0.0, 0.0, 0.0], 0.0, 4_800);
        let peak = out[2_400..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 1e-3, "{peak}");
        // 440 Hz at 48 kHz: one cycle every 109.09 samples.
        let crossings = out[2_400..]
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!((21..=22).contains(&crossings), "{crossings}");
    }

    #[test]
    fn modulators_are_not_heard_but_change_the_carrier() {
        let carrier = play(0, [0.5, 0.0, 0.0, 0.0, 0.0, 0.0], 0.0, 4_800);
        let modulated = play(0, [0.5, 0.3, 0.0, 0.0, 0.0, 0.0], 0.0, 4_800);
        let only_modulator = play(0, [0.0, 0.3, 0.0, 0.0, 0.0, 0.0], 0.0, 4_800);
        assert!(only_modulator.iter().all(|s| *s == 0.0));
        assert!(carrier != modulated);
        // Phase modulation leaves a carrier's peak level alone.
        let peak = modulated.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.5 + 1e-3);
    }

    #[test]
    fn levels_move_in_a_line_across_the_block() {
        let mut ops = Operators::new();
        let mut out = [0.0f32; 12];
        let step = [0.0; 6];
        // A zero step holds the phase at 0, where the sine is 0: shift to a
        // quarter cycle by modulating nothing and reading the levels back.
        ops.phase = [0.25; 6];
        ops.render(
            &ALGORITHMS[31],
            &step,
            &[1.2, 0.0, 0.0, 0.0, 0.0, 0.0],
            0.0,
            &mut out,
        );
        for (i, s) in out.iter().enumerate() {
            assert!((s - 0.1 * i as f32).abs() < 1e-5, "{i}: {s}");
        }
        assert_eq!(ops.level[0], 1.2);
    }

    #[test]
    fn feedback_brightens_a_lone_operator_and_stays_bounded() {
        let plain = play(31, [0.8, 0.0, 0.0, 0.0, 0.0, 0.8], 0.0, 4_800);
        let fed = play(31, [0.8, 0.0, 0.0, 0.0, 0.0, 0.8], 1.0, 4_800);
        assert!(plain != fed);
        assert!(fed.iter().all(|s| s.is_finite() && s.abs() <= 1.6 + 1e-3));
    }
}
