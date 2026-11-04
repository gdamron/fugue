//! The rhythm section: which of the two gates fire on a step.

/// How a step's coin tosses become gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// One toss: `gate_1` on heads (chance `bias`), `gate_2` on tails.
    Complementary,
    /// A toss per gate, each firing with chance `bias`.
    Independent,
    /// Drum-like: each gate has a metrical weight per position in a
    /// 16-step bar; strong positions always fire, and `bias` lets weaker
    /// ones in.
    Divider,
}

impl Mode {
    pub(super) fn from_position(position: u32) -> Option<Self> {
        match position {
            0 => Some(Self::Complementary),
            1 => Some(Self::Independent),
            2 => Some(Self::Divider),
            _ => None,
        }
    }
}

/// Steps in a divider bar.
pub(super) const BAR: usize = 16;

/// `gate_1`'s weights: a kick's, on the half-bar downbeats, then the
/// beats, then the offbeats.
const LOW_WEIGHTS: [f32; BAR] = [
    1.0, 0.1, 0.3, 0.1, 0.6, 0.1, 0.3, 0.15, 1.0, 0.1, 0.3, 0.1, 0.6, 0.1, 0.4, 0.2,
];

/// `gate_2`'s weights: a snare's, on the backbeats, then the offbeats.
const HIGH_WEIGHTS: [f32; BAR] = [
    0.05, 0.1, 0.3, 0.1, 1.0, 0.1, 0.4, 0.2, 0.05, 0.1, 0.3, 0.1, 1.0, 0.15, 0.4, 0.3,
];

/// Whether `gate_1` and `gate_2` fire on a step at `position` in the
/// divider bar, from the step's two coin tosses.
pub(super) fn fires(mode: Mode, bias: f32, coins: [f32; 2], position: usize) -> [bool; 2] {
    match mode {
        Mode::Complementary => {
            let heads = coins[0] < bias;
            [heads, !heads]
        }
        Mode::Independent => [coins[0] < bias, coins[1] < bias],
        Mode::Divider => {
            let position = position % BAR;
            // A weight's chance is weight^k: every position fires at bias
            // 1, a weight's own value at 0.5, only the full-weight ones at
            // 0. Computed per step, not per sample.
            let k = (1.0 - bias) / bias.max(f32::MIN_POSITIVE);
            let chance = |weight: f32| if weight >= 1.0 { 1.0 } else { weight.powf(k) };
            [
                coins[0] < chance(LOW_WEIGHTS[position]),
                coins[1] < chance(HIGH_WEIGHTS[position]),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complementary_fires_exactly_one_gate() {
        for coin in [0.0, 0.3, 0.5, 0.99] {
            let [one, two] = fires(Mode::Complementary, 0.5, [coin, 0.0], 0);
            assert_ne!(one, two);
        }
    }

    #[test]
    fn divider_keeps_its_strong_beats_at_any_bias() {
        for bias in [0.0, 0.5, 0.9] {
            assert_eq!(fires(Mode::Divider, bias, [0.999, 0.999], 0), [true, false]);
            assert_eq!(fires(Mode::Divider, bias, [0.999, 0.999], 4), [false, true]);
        }
        // At bias 0 nothing else plays; at 1 everything does.
        assert_eq!(fires(Mode::Divider, 0.0, [0.0, 0.0], 1), [false, false]);
        assert_eq!(fires(Mode::Divider, 1.0, [0.999, 0.999], 1), [true, true]);
    }
}
