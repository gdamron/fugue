//! The Adsr's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const ATTACK: ControlIndex = ControlIndex(0);
pub(super) const DECAY: ControlIndex = ControlIndex(1);
pub(super) const SUSTAIN: ControlIndex = ControlIndex(2);
pub(super) const RELEASE: ControlIndex = ControlIndex(3);

/// A stage time in seconds: 0 to 10 on a slider, any non-negative time held.
const fn time(key: &'static str, default: f32, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number {
            min: 0.0,
            max: 10.0,
        },
        RtValue::F32(default),
        description,
    )
    .unit("s")
    .precision(3)
    .clamped(0.0, f32::MAX)
}

const DECLS: &[ControlDecl] = &[
    time("attack", 0.01, "Attack time in seconds"),
    time("decay", 0.1, "Decay time in seconds"),
    ControlDecl::new(
        "sustain",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.7),
        "Sustain level",
    )
    .clamped(0.0, 1.0),
    time("release", 0.2, "Release time in seconds"),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every envelope's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
