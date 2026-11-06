//! The 32 ways six FM operators can be wired together.
//!
//! The topologies follow Mutable Instruments' `plaits/dsp/fm/algorithms.cc`
//! (MIT, Copyright 2021 Emilie Gillet), which describes the classic
//! six-operator layouts. They are written out here as plain wiring (which
//! operator modulates which, which operators are heard, where the feedback
//! loop sits), not as Plaits' compiled render calls. See
//! THIRD_PARTY_NOTICES.md.
//!
//! Operators are numbered 1 to 6. A modulator always has a higher number
//! than the operator it modulates, so rendering from operator 6 down to 1
//! computes every modulator before the operators it feeds.

/// How many operators a voice has.
pub(in crate::modules::macro_oscillator) const OPERATORS: usize = 6;

/// How many algorithms there are.
pub(in crate::modules::macro_oscillator) const ALGORITHM_COUNT: usize = 32;

/// One algorithm's wiring, by operator index (0 is operator 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) struct Algorithm {
    /// Which operators are heard: bit `k` is operator `k + 1`.
    pub carriers: u8,
    /// For each operator, which operators modulate it (same bits).
    pub modulators: [u8; OPERATORS],
    /// The feedback loop: the operator whose output is fed back, and the
    /// operator whose phase it modulates.
    pub feedback_from: usize,
    pub feedback_to: usize,
}

impl Algorithm {
    /// Whether operator `op` (0-based) is heard.
    pub fn is_carrier(&self, op: usize) -> bool {
        self.carriers & (1 << op) != 0
    }

    /// How many operators are heard.
    #[cfg(test)]
    pub fn carrier_count(&self) -> u32 {
        self.carriers.count_ones()
    }
}

/// Builds an algorithm from 1-based operator numbers: the carriers, each
/// `(operator, [its modulators])` link, and the feedback `(from, to)`.
const fn wire(carriers: &[u8], links: &[(u8, &[u8])], feedback: (u8, u8)) -> Algorithm {
    let mut mask = 0u8;
    let mut i = 0;
    while i < carriers.len() {
        mask |= 1 << (carriers[i] - 1);
        i += 1;
    }
    let mut modulators = [0u8; OPERATORS];
    let mut l = 0;
    while l < links.len() {
        let (op, sources) = links[l];
        let mut s = 0;
        while s < sources.len() {
            modulators[(op - 1) as usize] |= 1 << (sources[s] - 1);
            s += 1;
        }
        l += 1;
    }
    Algorithm {
        carriers: mask,
        modulators,
        feedback_from: (feedback.0 - 1) as usize,
        feedback_to: (feedback.1 - 1) as usize,
    }
}

/// The algorithms, numbered 1 to 32 in the classic order.
pub(in crate::modules::macro_oscillator) static ALGORITHMS: [Algorithm; ALGORITHM_COUNT] = [
    // 1-4: two stacks.
    wire(
        &[1, 3],
        &[(1, &[2]), (3, &[4]), (4, &[5]), (5, &[6])],
        (6, 6),
    ),
    wire(
        &[1, 3],
        &[(1, &[2]), (3, &[4]), (4, &[5]), (5, &[6])],
        (2, 2),
    ),
    wire(
        &[1, 4],
        &[(1, &[2]), (2, &[3]), (4, &[5]), (5, &[6])],
        (6, 6),
    ),
    wire(
        &[1, 4],
        &[(1, &[2]), (2, &[3]), (4, &[5]), (5, &[6])],
        (4, 6),
    ),
    // 5-6: three pairs.
    wire(&[1, 3, 5], &[(1, &[2]), (3, &[4]), (5, &[6])], (6, 6)),
    wire(&[1, 3, 5], &[(1, &[2]), (3, &[4]), (5, &[6])], (5, 6)),
    // 7-9: a pair and a branching stack.
    wire(&[1, 3], &[(1, &[2]), (3, &[4, 5]), (5, &[6])], (6, 6)),
    wire(&[1, 3], &[(1, &[2]), (3, &[4, 5]), (5, &[6])], (4, 4)),
    wire(&[1, 3], &[(1, &[2]), (3, &[4, 5]), (5, &[6])], (2, 2)),
    // 10-11: a three-stack and a forked pair.
    wire(&[1, 4], &[(1, &[2]), (2, &[3]), (4, &[5, 6])], (3, 3)),
    wire(&[1, 4], &[(1, &[2]), (2, &[3]), (4, &[5, 6])], (6, 6)),
    // 12-13: a pair and a carrier with three modulators.
    wire(&[1, 3], &[(1, &[2]), (3, &[4, 5, 6])], (2, 2)),
    wire(&[1, 3], &[(1, &[2]), (3, &[4, 5, 6])], (6, 6)),
    // 14-15: a pair and a forked stack.
    wire(&[1, 3], &[(1, &[2]), (3, &[4]), (4, &[5, 6])], (6, 6)),
    wire(&[1, 3], &[(1, &[2]), (3, &[4]), (4, &[5, 6])], (2, 2)),
    // 16-18: one carrier, three branches.
    wire(&[1], &[(1, &[2, 3, 5]), (3, &[4]), (5, &[6])], (6, 6)),
    wire(&[1], &[(1, &[2, 3, 5]), (3, &[4]), (5, &[6])], (2, 2)),
    wire(&[1], &[(1, &[2, 3, 4]), (4, &[5]), (5, &[6])], (3, 3)),
    // 19-25: more carriers sharing modulators.
    wire(
        &[1, 4, 5],
        &[(1, &[2]), (2, &[3]), (4, &[6]), (5, &[6])],
        (6, 6),
    ),
    wire(&[1, 2, 4], &[(1, &[3]), (2, &[3]), (4, &[5, 6])], (3, 3)),
    wire(
        &[1, 2, 4, 5],
        &[(1, &[3]), (2, &[3]), (4, &[6]), (5, &[6])],
        (3, 3),
    ),
    wire(
        &[1, 3, 4, 5],
        &[(1, &[2]), (3, &[6]), (4, &[6]), (5, &[6])],
        (6, 6),
    ),
    wire(&[1, 2, 4, 5], &[(2, &[3]), (4, &[6]), (5, &[6])], (6, 6)),
    wire(&[1, 2, 3, 4, 5], &[(3, &[6]), (4, &[6]), (5, &[6])], (6, 6)),
    wire(&[1, 2, 3, 4, 5], &[(4, &[6]), (5, &[6])], (6, 6)),
    // 26-27: two carriers' stacks and a free carrier.
    wire(&[1, 2, 4], &[(2, &[3]), (4, &[5, 6])], (6, 6)),
    wire(&[1, 2, 4], &[(2, &[3]), (4, &[5, 6])], (3, 3)),
    // 28-32: mostly carriers, toward an organ.
    wire(&[1, 3, 6], &[(1, &[2]), (3, &[4]), (4, &[5])], (5, 5)),
    wire(&[1, 2, 3, 5], &[(3, &[4]), (5, &[6])], (6, 6)),
    wire(&[1, 2, 3, 6], &[(3, &[4]), (4, &[5])], (5, 5)),
    wire(&[1, 2, 3, 4, 5], &[(5, &[6])], (6, 6)),
    wire(&[1, 2, 3, 4, 5, 6], &[], (6, 6)),
];

/// Whether operator `op` (0-based) modulates another in `algorithm`.
pub(in crate::modules::macro_oscillator) fn is_modulator(algorithm: &Algorithm, op: usize) -> bool {
    algorithm.modulators.iter().any(|m| m & (1 << op) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operator_is_heard_or_modulates_and_modulators_sit_higher() {
        for (n, algorithm) in ALGORITHMS.iter().enumerate() {
            for op in 0..OPERATORS {
                let heard = algorithm.is_carrier(op);
                let modulates = is_modulator(algorithm, op);
                assert!(
                    heard != modulates,
                    "algorithm {}: operator {}",
                    n + 1,
                    op + 1
                );
                // Every modulator of `op` has a higher number.
                assert_eq!(algorithm.modulators[op] & ((1 << (op + 1)) - 1), 0);
            }
            assert!(algorithm.feedback_from < OPERATORS && algorithm.feedback_to < OPERATORS);
        }
    }

    #[test]
    fn the_carrier_counts_run_from_one_to_six() {
        let counts: Vec<u32> = ALGORITHMS.iter().map(Algorithm::carrier_count).collect();
        assert_eq!(
            counts,
            [
                2, 2, 2, 2, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 3, 3, 4, 4, 4, 5, 5, 3, 3, 3,
                4, 4, 5, 6
            ]
        );
    }

    #[test]
    fn neighbouring_pairs_differ_only_in_feedback() {
        for (a, b) in [
            (1, 2),
            (3, 4),
            (5, 6),
            (7, 8),
            (8, 9),
            (10, 11),
            (12, 13),
            (14, 15),
            (16, 17),
            (26, 27),
        ] {
            let (a, b) = (ALGORITHMS[a - 1], ALGORITHMS[b - 1]);
            assert_eq!((a.carriers, a.modulators), (b.carriers, b.modulators));
            assert_ne!(
                (a.feedback_from, a.feedback_to),
                (b.feedback_from, b.feedback_to)
            );
        }
    }
}
