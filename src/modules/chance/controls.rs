//! The chance module's declared controls. Every one changes only as the
//! audio thread applies a request, so a write lands between samples and
//! takes effect from the next step.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const MODE: ControlIndex = ControlIndex(0);
pub(super) const BIAS: ControlIndex = ControlIndex(1);
pub(super) const REPETITION: ControlIndex = ControlIndex(2);
pub(super) const LOOP_LENGTH: ControlIndex = ControlIndex(3);
pub(super) const SEED: ControlIndex = ControlIndex(4);

/// The rhythm modes, in [`super::rhythm::Mode`] order.
pub(super) const MODES: &[&str] = &["complementary", "independent", "divider"];

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
        "repetition",
        0.0,
        "0: every step new; middle: a loop that mutates; 1: a locked loop",
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
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
