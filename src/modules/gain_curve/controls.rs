//! The gain curve atom's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const THRESHOLD: ControlIndex = ControlIndex(0);
pub(super) const RATIO: ControlIndex = ControlIndex(1);
pub(super) const KNEE: ControlIndex = ControlIndex(2);

/// The `threshold` range, in dB.
pub(super) const MIN_THRESHOLD: f32 = -80.0;
pub(super) const MAX_THRESHOLD: f32 = 0.0;
/// The `ratio` range.
pub(super) const MIN_RATIO: f32 = 1.0;
pub(super) const MAX_RATIO: f32 = 50.0;
/// The widest `knee`, in dB.
pub(super) const MAX_KNEE: f32 = 24.0;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "threshold",
        DeclKind::Number {
            min: MIN_THRESHOLD,
            max: MAX_THRESHOLD,
        },
        RtValue::F32(-20.0),
        "The level where compression starts, in dBFS",
    )
    .unit("dB")
    .clamped(MIN_THRESHOLD, MAX_THRESHOLD),
    ControlDecl::new(
        "ratio",
        DeclKind::Number {
            min: MIN_RATIO,
            max: MAX_RATIO,
        },
        RtValue::F32(4.0),
        "dB over the threshold in per dB out (1 leaves the level alone)",
    )
    .clamped(MIN_RATIO, MAX_RATIO)
    .log(),
    ControlDecl::new(
        "knee",
        DeclKind::Number {
            min: 0.0,
            max: MAX_KNEE,
        },
        RtValue::F32(6.0),
        "Width of the soft knee around the threshold, in dB (0 is a hard knee)",
    )
    .unit("dB")
    .clamped(0.0, MAX_KNEE),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
