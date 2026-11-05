//! A smear: four allpasses in series per channel, blended in by an amount.
//!
//! Ported from Mutable Instruments' `clouds/dsp/fx/diffuser.h` (MIT,
//! Copyright 2014 Emilie Gillet): the same lengths (scaled from 32 kHz to
//! the sample rate) and coefficient, built from Fugue's textbook
//! [`Allpass`]. See THIRD_PARTY_NOTICES.md.

use crate::dsp::Allpass;

/// Each channel's allpass lengths, in frames at 32 kHz.
const LEFT: [usize; 4] = [126, 180, 269, 444];
const RIGHT: [usize; 4] = [151, 205, 245, 405];
/// The original feeds the delayed sample forward at +0.625; [`Allpass`]
/// writes the opposite sign, so it takes -0.625 for the same filter.
const COEFFICIENT: f32 = -0.625;

pub struct Diffuser {
    left: [Allpass; 4],
    right: [Allpass; 4],
}

impl Diffuser {
    pub fn new(sample_rate: u32) -> Self {
        let scale = sample_rate as f32 / 32_000.0;
        let chain = |lengths: [usize; 4]| {
            lengths.map(|frames| {
                Allpass::new(
                    ((frames as f32 * scale).round() as usize).max(1),
                    COEFFICIENT,
                )
            })
        };
        Self {
            left: chain(LEFT),
            right: chain(RIGHT),
        }
    }

    /// One frame, `amount` (0 to 1) of it diffused. The allpasses run even
    /// at 0, so raising the amount blends in a chain already settled.
    #[inline]
    pub fn tick(&mut self, left: f32, right: f32, amount: f32) -> (f32, f32) {
        let wet_left = self.left.iter_mut().fold(left, |x, stage| stage.tick(x));
        let wet_right = self.right.iter_mut().fold(right, |x, stage| stage.tick(x));
        (
            left + amount * (wet_left - left),
            right + amount * (wet_right - right),
        )
    }
}
