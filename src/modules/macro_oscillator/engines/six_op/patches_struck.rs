//! The first half of the patch set: keys, then bells and mallets.
//!
//! Authored for Fugue: every value was chosen here, by ear and by
//! measurement, in the units [`super::patch`] describes. Operators are
//! listed 1 to 6; see [`super::algorithms`] for who modulates whom.

use super::envelope::FLOOR_DB;
use super::patch::{decaying, lfo, release, struck, Op, Patch};

/// Keys: patches 0 to 7.
// One operator to a line, so a patch reads as a table.
#[rustfmt::skip]
pub(super) const KEYS: [Patch; 8] = [
    Patch::new(
        "glass piano",
        5,
        0.2,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 1.2, -24.0, 7.0), 0.35)).keys(0.0, -1.0).rate_scale(0.4),
            Op::new(1.0, 2.0, struck(0.001, 0.5, -8.0, 4.0)).keys(1.0, -2.0).rate_scale(0.5).velocity(9.0),
            Op::new(1.0, -10.0, release(struck(0.001, 1.2, -24.0, 7.0), 0.35)).detune(2.0).rate_scale(0.4),
            Op::new(3.0, -2.0, struck(0.001, 0.3, -14.0, 2.5)).keys(0.0, -3.0).velocity(10.0),
            Op::new(7.0, -24.0, release(decaying(0.001, 0.8), 0.35)).keys(0.0, -3.0),
            Op::new(1.0, -6.0, decaying(0.001, 0.8)),
        ],
    )
    .gain(2.5),
    Patch::new(
        "soft tine keys",
        5,
        0.0,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 2.0, -20.0, 9.0), 0.3)).rate_scale(0.3),
            Op::new(1.0, -1.0, struck(0.001, 0.9, -18.0, 5.0)).keys(0.0, -2.0).velocity(12.0),
            Op::new(1.0, -12.0, release(struck(0.001, 0.5, -30.0, 4.0), 0.3)).rate_scale(0.3),
            Op::new(14.0, -6.0, decaying(0.001, 0.35)).keys(0.0, -4.0).velocity(14.0),
            Op::new(1.0, -16.0, release(struck(0.002, 3.0, -12.0, 10.0), 0.3)),
            Op::new(1.0, -12.0, decaying(0.002, 1.5)),
        ],
    )
    .gain(1.0),
    Patch::new(
        "bright tine keys",
        5,
        0.4,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 1.6, -18.0, 8.0), 0.3)).rate_scale(0.3).tremolo(1.0),
            Op::new(1.0, 4.0, struck(0.001, 0.7, -15.0, 5.0)).keys(0.0, -2.0).velocity(12.0),
            Op::new(1.0, -11.0, release(struck(0.001, 0.6, -24.0, 5.0), 0.3)).detune(4.0).tremolo(1.0),
            Op::new(14.0, 0.0, decaying(0.001, 0.4)).keys(0.0, -4.0).velocity(14.0),
            Op::new(2.0, -20.0, release(decaying(0.001, 1.5), 0.3)).tremolo(1.0),
            Op::new(1.0, -4.0, decaying(0.001, 0.9)),
        ],
    )
    .lfo(lfo(5.5, 0.0, 0.0, 3.0))
    .gain(4.5),
    Patch::new(
        "reed keys",
        1,
        0.5,
        [
            Op::new(1.0, -8.0, release(struck(0.002, 3.0, -10.0, 8.0), 0.25)).rate_scale(0.3),
            Op::new(1.0, 7.0, struck(0.002, 1.0, -10.0, 6.0)).keys(0.0, -3.0).velocity(14.0),
            Op::new(2.0, -18.0, release(decaying(0.002, 1.5), 0.25)),
            Op::new(1.0, 3.0, struck(0.002, 0.6, -12.0, 4.0)),
            Op::new(3.0, -6.0, decaying(0.001, 0.2)).velocity(12.0),
            Op::new(1.0, -6.0, struck(0.002, 1.0, -6.0, 6.0)),
        ],
    )
    .gain(5.0),
    Patch::new(
        "funky pluck keys",
        5,
        0.6,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 0.8, -20.0, 3.0), 0.06)),
            Op::new(3.0, 6.0, struck(0.001, 0.3, -12.0, 2.0)).keys(0.0, -2.0).velocity(12.0),
            Op::new(3.0, -14.0, release(decaying(0.001, 1.5), 0.06)),
            Op::new(1.0, 3.0, decaying(0.001, 0.6)).velocity(10.0),
            Op::new(5.0, -24.0, release(decaying(0.001, 0.25), 0.06)).detune(3.0),
            Op::new(1.0, -6.0, decaying(0.001, 0.3)),
        ],
    )
    .gain(5.5),
    Patch::new(
        "quill keys",
        2,
        0.55,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 1.0, -15.0, 5.0), 0.15)).rate_scale(0.4),
            Op::new(1.0, 9.0, struck(0.001, 0.5, -15.0, 4.0)).keys(0.0, -2.0).velocity(8.0),
            Op::new(2.0, -12.0, release(struck(0.001, 0.8, -15.0, 4.0), 0.15)).detune(-3.0),
            Op::new(4.0, 2.0, struck(0.001, 0.3, -20.0, 3.0)).keys(0.0, -3.0),
            Op::new(1.0, -6.0, decaying(0.001, 1.0)),
            Op::new(2.0, -12.0, decaying(0.001, 0.5)),
        ],
    )
    .gain(5.0),
    Patch::new(
        "toy piano",
        5,
        0.0,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 0.4, -18.0, 2.5), 0.4)).rate_scale(0.3),
            Op::new(1.0, 3.0, struck(0.001, 0.2, -6.0, 2.5)).velocity(10.0),
            Op::new(6.27, -18.0, release(decaying(0.001, 0.5), 0.4)).keys(0.0, -2.0),
            Op::new(1.0, -6.0, struck(0.001, 0.2, -10.0, 2.0)),
            Op::new(2.76, -16.0, release(decaying(0.001, 0.9), 0.4)),
            Op::new(1.0, -12.0, decaying(0.001, 0.5)),
        ],
    )
    .gain(7.5),
    Patch::new(
        "music box",
        5,
        0.0,
        [
            Op::new(1.0, -8.0, struck(0.001, 0.6, -12.0, 4.5)).rate_scale(0.4),
            Op::new(4.0, -2.0, struck(0.001, 0.4, -12.0, 4.0)).keys(0.0, -3.0),
            Op::new(3.0, -18.0, decaying(0.001, 1.5)).detune(5.0),
            Op::new(1.0, -10.0, struck(0.001, 0.3, -10.0, 3.0)),
            Op::new(5.4, -26.0, decaying(0.001, 0.7)),
            Op::new(1.0, -20.0, decaying(0.001, 0.5)),
        ],
    )
    .gain(6.0),
];

/// Bells and mallets: patches 8 to 15.
// One operator to a line, so a patch reads as a table.
#[rustfmt::skip]
pub(super) const BELLS: [Patch; 8] = [
    Patch::new(
        "tubular bell",
        5,
        0.1,
        [
            Op::new(1.0, -8.0, struck(0.002, 3.0, -12.0, 14.0)),
            Op::new(3.5, 2.0, struck(0.002, 1.5, -15.0, 8.0)).keys(0.0, -2.0),
            Op::new(2.0, -12.0, struck(0.002, 2.0, -12.0, 10.0)).detune(7.0),
            Op::new(5.19, -2.0, struck(0.002, 0.8, -18.0, 5.0)),
            Op::new(4.0, -18.0, decaying(0.002, 4.0)).detune(-9.0),
            Op::new(1.41, -6.0, decaying(0.002, 3.0)),
        ],
    )
    .gain(2.5),
    Patch::new(
        "glockenspiel",
        5,
        0.0,
        [
            Op::new(1.0, -8.0, struck(0.001, 0.8, -14.0, 3.5)).rate_scale(0.3),
            Op::new(3.5, -2.0, struck(0.001, 0.3, -14.0, 3.0)).keys(0.0, -3.0).velocity(10.0),
            Op::new(2.76, -16.0, decaying(0.001, 1.2)).rate_scale(0.3),
            Op::new(1.0, -12.0, struck(0.001, 0.2, -10.0, 2.0)),
            Op::new(5.4, -24.0, decaying(0.001, 0.6)),
            Op::new(1.0, -18.0, decaying(0.001, 0.3)),
        ],
    )
    .gain(5.5),
    Patch::new(
        "vibraphone",
        5,
        0.0,
        [
            Op::new(1.0, -8.0, release(struck(0.001, 2.5, -10.0, 8.0), 0.4)).tremolo(1.0),
            Op::new(4.0, -4.0, struck(0.001, 0.3, -12.0, 6.0)).velocity(10.0),
            Op::new(4.0, -20.0, release(decaying(0.001, 1.0), 0.4)).tremolo(1.0),
            Op::new(1.0, -10.0, struck(0.001, 0.3, -10.0, 4.0)),
            Op::new(10.0, -28.0, decaying(0.001, 0.08)),
            Op::new(1.0, -6.0, decaying(0.001, 0.05)),
        ],
    )
    .lfo(lfo(5.5, 0.0, 0.0, 4.0))
    .gain(5.5),
    Patch::new(
        "marimba",
        5,
        0.0,
        [
            Op::new(1.0, -6.0, struck(0.001, 0.25, -18.0, 1.6)).rate_scale(0.5),
            Op::new(4.0, -3.0, struck(0.001, 0.08, -18.0, 1.5)).keys(0.0, -3.0).velocity(12.0),
            Op::new(4.0, -22.0, decaying(0.001, 0.25)).rate_scale(0.5),
            Op::new(1.0, -12.0, struck(0.001, 0.1, -12.0, 1.2)),
            Op::new(10.0, -32.0, decaying(0.001, 0.05)),
            Op::new(1.0, FLOOR_DB, decaying(0.001, 0.05)),
        ],
    )
    .gain(9.0),
    Patch::new(
        "kalimba",
        5,
        0.0,
        [
            Op::new(1.0, -6.0, struck(0.001, 0.6, -15.0, 3.0)).rate_scale(0.3),
            Op::new(1.0, -3.0, struck(0.001, 0.15, -14.0, 3.0)).velocity(12.0),
            Op::new(5.95, -22.0, decaying(0.001, 0.15)),
            Op::new(1.0, -12.0, struck(0.001, 0.1, -12.0, 2.0)),
            Op::new(1.0, -20.0, struck(0.001, 0.6, -15.0, 3.0)).detune(6.0),
            Op::new(2.0, -12.0, struck(0.001, 0.3, -10.0, 3.0)),
        ],
    )
    .sweep(0.3, 0.05)
    .gain(4.0),
    Patch::new(
        "steel pan",
        5,
        0.2,
        [
            Op::new(1.0, -8.0, release(struck(0.003, 1.2, -12.0, 4.0), 1.5)),
            Op::new(1.0, 2.0, struck(0.002, 0.3, -6.0, 3.5)).velocity(10.0),
            Op::new(2.0, -12.0, release(struck(0.003, 1.0, -12.0, 3.5), 1.5)).detune(12.0),
            Op::new(3.0, -4.0, struck(0.002, 0.4, -10.0, 3.5)),
            Op::new(3.0, -20.0, decaying(0.002, 1.5)).detune(-10.0),
            Op::new(1.0, -8.0, struck(0.002, 0.6, -6.0, 4.0)),
        ],
    )
    .sweep(0.2, 0.08)
    .gain(4.5),
    Patch::new(
        "gong",
        1,
        0.5,
        [
            Op::new(1.0, -8.0, release(struck(0.01, 4.0, -10.0, 20.0), 12.0)).tremolo(1.0),
            Op::new(1.41, 6.0, struck(0.01, 2.0, -6.0, 15.0)),
            Op::new(0.71, -10.0, release(struck(0.02, 5.0, -8.0, 25.0), 12.0)).tremolo(1.0),
            Op::new(2.37, 4.0, struck(0.05, 6.0, -12.0, 20.0)),
            Op::new(3.17, 0.0, decaying(0.01, 8.0)),
            Op::new(1.0, -6.0, decaying(0.01, 10.0)),
        ],
    )
    .lfo(lfo(0.4, 0.0, 0.0, 1.5))
    .gain(4.0),
    Patch::new(
        "crystal bell",
        5,
        0.0,
        [
            Op::new(1.0, -8.0, release(struck(0.002, 1.5, -10.0, 6.0), 5.0)),
            Op::new(7.0, -3.0, decaying(0.002, 1.0)).keys(0.0, -2.0),
            Op::new(4.0, -14.0, decaying(0.002, 3.0)).detune(4.0),
            Op::new(1.0, -10.0, decaying(0.002, 2.0)),
            Op::new(1.0, -14.0, release(struck(0.002, 1.5, -10.0, 6.0), 5.0)).detune(-4.0),
            Op::new(5.0, -9.0, decaying(0.002, 1.2)),
        ],
    )
    .gain(2.0),
];
