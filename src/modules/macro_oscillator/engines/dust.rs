//! Ported from Mutable Instruments' `plaits/dsp/noise/dust.h` (MIT,
//! Copyright 2016 Emilie Gillet). See THIRD_PARTY_NOTICES.md.

use crate::dsp::Rng;

/// Sparse random impulses: on average `frequency` of the samples (0 to 1)
/// carry an impulse of random height from 0 to 1, and the rest are 0.
#[inline]
pub(super) fn dust(rng: &mut Rng, frequency: f32) -> f32 {
    let u = rng.unit();
    if u < frequency {
        u / frequency
    } else {
        0.0
    }
}
