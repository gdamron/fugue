//! The follower atom's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const ATTACK: ControlIndex = ControlIndex(0);
pub(super) const RELEASE: ControlIndex = ControlIndex(1);
pub(super) const SIDECHAIN_MIX: ControlIndex = ControlIndex(2);
pub(super) const HIGHPASS: ControlIndex = ControlIndex(3);

/// The longest `attack`, in seconds.
pub(super) const MAX_ATTACK: f32 = 1.0;
/// The `release` range, in seconds.
pub(super) const MIN_RELEASE: f32 = 0.001;
pub(super) const MAX_RELEASE: f32 = 5.0;
/// The highest `highpass` cutoff, in hertz.
pub(super) const MAX_HIGHPASS: f32 = 2000.0;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "attack",
        DeclKind::Number {
            min: 0.0,
            max: MAX_ATTACK,
        },
        RtValue::F32(0.01),
        "Seconds for a rising envelope to cover 63% of a step (0 follows at once)",
    )
    .unit("s")
    .clamped(0.0, MAX_ATTACK),
    ControlDecl::new(
        "release",
        DeclKind::Number {
            min: MIN_RELEASE,
            max: MAX_RELEASE,
        },
        RtValue::F32(0.1),
        "Seconds for a falling envelope to cover 63% of a step",
    )
    .unit("s")
    .clamped(MIN_RELEASE, MAX_RELEASE)
    .log(),
    ControlDecl::new(
        "sidechain_mix",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "What is followed: 0 the audio inputs, 1 the sidechain input",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "highpass",
        DeclKind::Number {
            min: 0.0,
            max: MAX_HIGHPASS,
        },
        RtValue::F32(0.0),
        "High-pass cutoff on what is followed, so lows move the envelope less (0 is off)",
    )
    .unit("Hz")
    .clamped(0.0, MAX_HIGHPASS),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
