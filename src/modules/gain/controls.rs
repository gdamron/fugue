//! The gain atom's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const DECIBELS: ControlIndex = ControlIndex(0);
pub(super) const DECIBELS_MOD_DEPTH: ControlIndex = ControlIndex(1);
pub(super) const INVERT: ControlIndex = ControlIndex(2);

/// The `decibels` control's range.
pub(super) const MIN_DECIBELS: f32 = -96.0;
pub(super) const MAX_DECIBELS: f32 = 24.0;
/// The most `decibels_mod_depth` takes.
pub(super) const MAX_DEPTH: f32 = 96.0;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "decibels",
        DeclKind::Number {
            min: MIN_DECIBELS,
            max: MAX_DECIBELS,
        },
        RtValue::F32(0.0),
        "The gain in dB: 0 passes the signal unchanged, above 0 boosts it",
    )
    .unit("dB")
    .clamped(MIN_DECIBELS, MAX_DECIBELS),
    ControlDecl::new(
        "decibels_mod_depth",
        DeclKind::Number {
            min: 0.0,
            max: MAX_DEPTH,
        },
        RtValue::F32(1.0),
        "dB added per unit of decibels_mod (1 reads it directly in dB)",
    )
    .clamped(0.0, MAX_DEPTH),
    ControlDecl::new(
        "invert",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Flip the output's sign (for negative feedback)",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
