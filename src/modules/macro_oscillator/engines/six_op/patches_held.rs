//! The second half of the patch set: basses and plucks, then organs, pads,
//! winds and brass.
//!
//! Authored for Fugue, like [`super::patches_struck`].

use super::envelope::FLOOR_DB;
use super::patch::{decaying, lfo, release, struck, sustained, Op, Patch};

/// Basses and plucks: patches 16 to 23.
// One operator to a line, so a patch reads as a table.
#[rustfmt::skip]
pub(super) const PLUCKS: [Patch; 8] = [
    Patch::new(
        "solid bass",
        5,
        0.0,
        [
            Op::new(1.0, -6.0, release(struck(0.001, 1.0, -8.0, 6.0), 0.08)),
            Op::new(1.0, 6.0, struck(0.001, 0.4, -8.0, 4.0)).keys(0.0, -3.0).velocity(10.0),
            Op::new(0.5, -10.0, release(struck(0.002, 2.0, -6.0, 8.0), 0.08)),
            Op::new(1.0, -6.0, decaying(0.001, 0.5)),
            Op::new(2.0, -24.0, decaying(0.001, 0.2)),
            Op::new(3.0, -6.0, decaying(0.001, 0.1)),
        ],
    )
    .gain(3.5),
    Patch::new(
        "slap bass",
        5,
        0.3,
        [
            Op::new(1.0, -6.0, release(struck(0.001, 0.8, -10.0, 4.0), 0.1)),
            Op::new(1.0, 9.0, struck(0.001, 0.12, -14.0, 3.0)).velocity(14.0),
            Op::new(3.0, -18.0, release(decaying(0.001, 0.15), 0.1)),
            Op::new(5.0, 0.0, decaying(0.001, 0.05)),
            Op::new(0.5, -12.0, release(struck(0.001, 1.5, -8.0, 5.0), 0.1)),
            Op::new(1.0, -6.0, decaying(0.001, 0.6)),
        ],
    )
    .sweep(1.0, 0.03)
    .gain(3.5),
    Patch::new(
        "rubber bass",
        1,
        0.6,
        [
            Op::new(1.0, -6.0, release(sustained(0.002, 0.8, -6.0), 0.1)),
            Op::new(1.0, 10.0, struck(0.002, 0.25, -10.0, 2.0)).keys(0.0, -3.0).velocity(12.0),
            Op::new(1.0, -12.0, release(sustained(0.002, 0.8, -6.0), 0.1)).detune(-6.0),
            Op::new(0.5, 4.0, struck(0.002, 0.3, -8.0, 3.0)),
            Op::new(1.0, -3.0, sustained(0.002, 0.5, -6.0)),
            Op::new(1.0, -6.0, sustained(0.002, 0.5, -6.0)),
        ],
    )
    .gain(4.0),
    Patch::new(
        "sub pluck",
        5,
        0.0,
        [
            Op::new(1.0, -4.0, release(struck(0.002, 0.5, -12.0, 3.0), 0.1)),
            Op::new(1.0, -2.0, struck(0.002, 0.15, -12.0, 3.0)).velocity(10.0),
            Op::new(0.5, -8.0, release(struck(0.002, 0.8, -12.0, 4.0), 0.1)),
            Op::OFF,
            Op::OFF,
            Op::OFF,
        ],
    )
    .gain(2.5),
    Patch::new(
        "harp",
        5,
        0.0,
        [
            Op::new(1.0, -6.0, release(struck(0.002, 1.5, -15.0, 6.0), 2.5)).rate_scale(0.4),
            Op::new(1.0, 1.0, struck(0.002, 0.3, -12.0, 3.0)).keys(0.0, -2.0).velocity(10.0),
            Op::new(2.0, -16.0, release(struck(0.002, 0.8, -12.0, 4.0), 2.5)).rate_scale(0.4),
            Op::new(1.0, -6.0, decaying(0.002, 0.4)),
            Op::new(1.0, -14.0, release(struck(0.002, 1.5, -15.0, 6.0), 2.5)).detune(3.0),
            Op::new(3.0, -12.0, decaying(0.002, 0.2)),
        ],
    )
    .gain(2.0),
    Patch::new(
        "plucked silk",
        5,
        0.2,
        [
            Op::new(1.0, -6.0, release(struck(0.001, 0.6, -14.0, 4.0), 0.6)).rate_scale(0.4),
            Op::new(1.0, 6.0, struck(0.001, 0.15, -16.0, 2.5)).velocity(12.0),
            Op::new(3.0, -18.0, release(decaying(0.001, 0.5), 0.6)),
            Op::new(1.0, 0.0, decaying(0.001, 0.2)),
            Op::new(1.0, -16.0, release(struck(0.001, 0.6, -14.0, 4.0), 0.6)).detune(5.0),
            Op::new(2.0, -6.0, decaying(0.001, 0.4)),
        ],
    )
    .sweep(0.5, 0.04)
    .lfo(lfo(5.0, 0.6, 12.0, 0.0))
    .gain(4.0),
    Patch::new(
        "nylon guitar",
        5,
        0.0,
        [
            Op::new(1.0, -6.0, release(struck(0.002, 1.0, -14.0, 5.0), 0.3)).rate_scale(0.4),
            Op::new(1.0, 2.0, struck(0.002, 0.2, -15.0, 3.0)).keys(0.0, -2.0).velocity(10.0),
            Op::new(2.0, -18.0, release(decaying(0.002, 1.5), 0.3)),
            Op::new(1.0, -3.0, decaying(0.002, 0.2)),
            Op::new(1.0, -14.0, release(struck(0.002, 1.0, -14.0, 5.0), 0.3)).detune(2.0),
            Op::new(4.0, -10.0, decaying(0.002, 0.08)),
        ],
    )
    .gain(2.5),
    Patch::new(
        "twang",
        2,
        0.5,
        [
            Op::new(1.0, -6.0, release(struck(0.001, 0.4, -16.0, 2.5), 0.15)).rate_scale(0.4),
            Op::new(1.0, 8.0, struck(0.001, 0.1, -12.0, 2.0)).velocity(12.0),
            Op::new(3.0, -16.0, release(decaying(0.001, 0.4), 0.15)),
            Op::new(1.0, 0.0, decaying(0.001, 0.12)),
            Op::new(2.0, -6.0, decaying(0.001, 0.2)),
            Op::new(1.0, -16.0, decaying(0.001, 0.05)).fixed(3_200.0),
        ],
    )
    .gain(7.0),
];

/// Organs, pads, winds and brass: patches 24 to 31.
// One operator to a line, so a patch reads as a table. The metal drone's
// inharmonic ratios were chosen by ear; they only look like e and pi.
#[rustfmt::skip]
#[allow(clippy::approx_constant)]
pub(super) const HELD: [Patch; 8] = [
    Patch::new(
        "drawbar organ",
        32,
        0.0,
        [
            Op::new(1.0, -8.0, release(sustained(0.002, 0.05, 0.0), 0.05)),
            Op::new(2.0, -12.0, release(sustained(0.002, 0.05, 0.0), 0.05)),
            Op::new(3.0, -14.0, release(sustained(0.002, 0.05, 0.0), 0.05)),
            Op::new(4.0, -16.0, release(sustained(0.002, 0.05, 0.0), 0.05)),
            Op::new(6.0, -22.0, release(sustained(0.002, 0.05, 0.0), 0.05)),
            Op::new(8.0, -22.0, release(sustained(0.002, 0.05, 0.0), 0.05)),
        ],
    )
    .gain(2.5),
    Patch::new(
        "reed organ",
        5,
        0.3,
        [
            Op::new(1.0, -8.0, release(sustained(0.03, 0.3, -2.0), 0.15)).tremolo(1.0),
            Op::new(1.0, 5.0, sustained(0.03, 0.5, -3.0)),
            Op::new(2.0, -14.0, release(sustained(0.04, 0.3, -2.0), 0.15)).detune(3.0).tremolo(1.0),
            Op::new(1.0, 3.0, sustained(0.04, 0.5, -3.0)),
            Op::new(1.0, -12.0, release(sustained(0.03, 0.3, -2.0), 0.15)).detune(-4.0).tremolo(1.0),
            Op::new(3.0, -3.0, sustained(0.03, 0.5, -3.0)),
        ],
    )
    .lfo(lfo(6.0, 0.0, 0.0, 1.0))
    .gain(4.0),
    Patch::new(
        "warm pad",
        5,
        0.2,
        [
            Op::new(1.0, -8.0, release(sustained(0.6, 1.0, -3.0), 1.5)).detune(-5.0),
            Op::new(1.0, -2.0, release(sustained(1.2, 2.0, -6.0), 1.5)),
            Op::new(1.0, -8.0, release(sustained(0.6, 1.0, -3.0), 1.5)).detune(6.0),
            Op::new(2.0, -6.0, release(sustained(1.5, 2.0, -6.0), 1.5)),
            Op::new(0.5, -14.0, release(sustained(0.8, 1.0, -3.0), 1.5)),
            Op::new(1.0, -8.0, release(sustained(2.0, 2.0, -6.0), 1.5)),
        ],
    )
    .lfo(lfo(4.2, 0.8, 6.0, 0.0))
    .gain(-0.5),
    Patch::new(
        "glass pad",
        5,
        0.1,
        [
            Op::new(1.0, -8.0, release(sustained(0.4, 3.0, -6.0), 2.0)).tremolo(1.0),
            Op::new(3.0, -4.0, release(struck(0.4, 2.0, -12.0, 10.0), 2.0)),
            Op::new(2.0, -14.0, release(sustained(0.8, 3.0, -6.0), 2.0)).detune(7.0).tremolo(1.0),
            Op::new(7.0, -10.0, decaying(0.6, 3.0)),
            Op::new(1.0, -10.0, release(sustained(0.4, 3.0, -6.0), 2.0)).detune(-7.0).tremolo(1.0),
            Op::new(1.0, -6.0, release(sustained(1.5, 3.0, -6.0), 2.0)),
        ],
    )
    .lfo(lfo(0.3, 0.0, 0.0, 1.5))
    .gain(2.5),
    Patch::new(
        "brass section",
        22,
        0.5,
        [
            Op::new(1.0, -10.0, release(sustained(0.06, 0.5, -2.0), 0.2)),
            Op::new(1.0, 4.0, release(sustained(0.08, 0.6, -4.0), 0.2)).keys(0.0, -2.0).velocity(12.0),
            Op::new(1.0, -10.0, release(sustained(0.07, 0.5, -2.0), 0.2)).detune(7.0),
            Op::new(1.0, -10.0, release(sustained(0.07, 0.5, -2.0), 0.2)).detune(-7.0),
            Op::new(2.0, -20.0, release(sustained(0.06, 0.5, -2.0), 0.2)),
            Op::new(1.0, 6.0, release(sustained(0.1, 0.6, -6.0), 0.2)).keys(0.0, -2.0).velocity(12.0),
        ],
    )
    .sweep(-0.8, 0.06)
    .lfo(lfo(5.0, 0.6, 8.0, 0.0))
    .gain(1.0),
    Patch::new(
        "flute",
        5,
        0.9,
        [
            Op::new(1.0, -6.0, release(sustained(0.08, 0.3, -1.0), 0.15)).tremolo(1.0),
            Op::new(1.0, -6.0, release(sustained(0.1, 0.5, -4.0), 0.15)).velocity(10.0),
            Op::new(2.0, -24.0, release(sustained(0.06, 0.3, -1.0), 0.15)),
            Op::new(1.0, FLOOR_DB, decaying(0.001, 0.005)),
            Op::new(1.0, -30.0, release(decaying(0.01, 0.15), 0.15)),
            Op::new(7.0, 12.0, decaying(0.01, 0.15)),
        ],
    )
    .lfo(lfo(5.0, 0.3, 0.0, 0.5))
    .gain(4.0),
    Patch::new(
        "reed lead",
        1,
        0.3,
        [
            Op::new(1.0, -8.0, release(sustained(0.04, 0.4, -2.0), 0.12)),
            Op::new(2.0, 4.0, release(sustained(0.06, 0.5, -3.0), 0.12)).keys(0.0, -2.0).velocity(10.0),
            Op::new(1.0, -16.0, release(sustained(0.04, 0.4, -2.0), 0.12)),
            Op::new(1.0, 6.0, sustained(0.05, 0.5, -3.0)),
            Op::new(3.0, -3.0, sustained(0.05, 0.5, -3.0)),
            Op::new(1.0, -6.0, sustained(0.05, 0.5, -3.0)),
        ],
    )
    .lfo(lfo(5.2, 0.4, 10.0, 0.0).triangle())
    .gain(3.5),
    Patch::new(
        "metal drone",
        1,
        0.7,
        [
            Op::new(1.0, -8.0, release(sustained(1.5, 4.0, -4.0), 4.0)).tremolo(1.0),
            Op::new(1.414, 3.0, release(sustained(3.0, 6.0, -3.0), 4.0)),
            Op::new(0.5, -10.0, release(sustained(2.0, 4.0, -4.0), 4.0)).tremolo(1.0),
            Op::new(2.718, 2.0, release(sustained(4.0, 8.0, -6.0), 4.0)),
            Op::new(3.14, -2.0, release(sustained(3.0, 6.0, -3.0), 4.0)),
            Op::new(1.0, -4.0, release(sustained(2.0, 6.0, -3.0), 4.0)),
        ],
    )
    .lfo(lfo(0.15, 0.0, 3.0, 2.0))
    .gain(3.5),
];
