//! The chance module's declared controls. Every one changes only as the
//! audio thread applies a request, so a write lands between samples and
//! takes effect from the next step.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::music::scales::{KEY_NAMES, KEY_SPELLINGS, SCALE_NAMES};

pub(super) const MODE: ControlIndex = ControlIndex(0);
pub(super) const BIAS: ControlIndex = ControlIndex(1);
pub(super) const SPREAD: ControlIndex = ControlIndex(2);
pub(super) const VALUE_BIAS: ControlIndex = ControlIndex(3);
pub(super) const STEPS: ControlIndex = ControlIndex(4);
pub(super) const KEY: ControlIndex = ControlIndex(5);
pub(super) const SCALE: ControlIndex = ControlIndex(6);
pub(super) const LOW_NOTE: ControlIndex = ControlIndex(7);
pub(super) const RANGE: ControlIndex = ControlIndex(8);
pub(super) const REPETITION: ControlIndex = ControlIndex(9);
pub(super) const LOOP_LENGTH: ControlIndex = ControlIndex(10);
pub(super) const SEED: ControlIndex = ControlIndex(11);
pub(super) const RATE: ControlIndex = ControlIndex(12);
pub(super) const JITTER: ControlIndex = ControlIndex(13);

/// The rhythm modes, in [`super::rhythm::Mode`] order.
pub(super) const MODES: &[&str] = &["complementary", "independent", "divider"];

/// Steps per clock pulse, in [`super::timing::Rate`] order: a step every
/// 4th, 3rd or 2nd pulse, one per pulse, or 2 to 4 per pulse.
pub(super) const RATES: &[&str] = &["div4", "div3", "div2", "x1", "x2", "x3", "x4"];

/// The longest repetition loop, in steps.
pub(super) const MAX_LOOP: usize = 16;

/// The seed as a number: f32 controls hold integers exactly to 2^24.
const SEED_LIMIT: f32 = 16_777_216.0;

/// The seed of a module with none: it plays a stream from entropy.
pub(super) const UNSEEDED: f32 = -1.0;

const fn unit(key: &'static str, default: f32, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(default),
        description,
    )
    .clamped(0.0, 1.0)
}

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "mode",
        DeclKind::Choice(MODES),
        RtValue::U32(0),
        "Rhythm: complementary (one coin toss: gate_1 on heads, gate_2 on \
         tails), independent (a toss per gate) or divider (drum-like \
         patterns over 16 steps; bias adds weaker beats, so the gate rate \
         follows the beats' weights rather than bias, and a locked loop \
         whose length does not divide 16 repeats every lcm(length, 16) steps)",
    ),
    unit(
        "bias",
        0.5,
        "Rhythm: chance a gate fires on a step (complementary and independent)",
    ),
    unit(
        "spread",
        0.5,
        "Melody: width of the values' range, from always value_bias (0) to anywhere (1)",
    ),
    unit("value_bias", 0.5, "Melody: where the values lean, 0 to 1"),
    unit(
        "steps",
        0.5,
        "Melody: below 1/3 smooth, to 0.5 stepped, from 0.5 quantized to key \
         and scale, narrowing toward the scale's strongest degrees at 1 (its tonic, \
         fifth and third; a chromatic scale's are a major triad)",
    ),
    ControlDecl::new(
        "key",
        DeclKind::Choice(KEY_NAMES),
        RtValue::U32(0),
        "Tonic of the scale pitches are quantized to",
    )
    .spellings(KEY_SPELLINGS),
    ControlDecl::new(
        "scale",
        DeclKind::Choice(SCALE_NAMES),
        RtValue::U32(0),
        "Scale pitches are quantized to",
    ),
    ControlDecl::new(
        "low_note",
        DeclKind::Number {
            min: 0.0,
            max: 127.0,
        },
        RtValue::F32(48.0),
        "MIDI note a value of 0 plays",
    )
    .clamped(0.0, 127.0)
    .whole(),
    ControlDecl::new(
        "range",
        DeclKind::Number {
            min: 1.0,
            max: 48.0,
        },
        RtValue::F32(24.0),
        "Semitones a value of 1 lies above low_note",
    )
    .clamped(1.0, 48.0)
    .whole(),
    unit(
        "repetition",
        0.0,
        "0: every step new; middle: a loop that mutates; 1: a locked loop. \
         Shared by rhythm and melody",
    ),
    ControlDecl::new(
        "loop_length",
        DeclKind::Number {
            min: 1.0,
            max: MAX_LOOP as f32,
        },
        RtValue::F32(8.0),
        "Steps in the repetition loop",
    )
    .clamped(1.0, MAX_LOOP as f32)
    .whole(),
    ControlDecl::new(
        "seed",
        DeclKind::Number {
            min: UNSEEDED,
            max: SEED_LIMIT,
        },
        RtValue::F32(UNSEEDED),
        "RNG seed: same seed, same output; setting it restarts the stream. \
         -1 is no seed: a stream from entropy. Note: f32 controls carry ~24 \
         bits of integer precision; use module config JSON for full 64-bit seeds.",
    )
    .clamped(UNSEEDED, f32::MAX)
    .whole(),
    ControlDecl::new(
        "rate",
        DeclKind::Choice(RATES),
        RtValue::U32(3),
        "Steps per clock pulse: div4 to div2 step every few pulses, x2 to x4 \
         subdivide each pulse (from the second pulse, once the clock's \
         period is known)",
    ),
    unit(
        "jitter",
        0.0,
        "Rhythm: delays each fired gate by a random amount, up to half a step",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
