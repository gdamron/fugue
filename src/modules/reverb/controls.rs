//! The Reverb's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const ROOM_SIZE: ControlIndex = ControlIndex(0);
pub(super) const DECAY: ControlIndex = ControlIndex(1);
pub(super) const DAMPING: ControlIndex = ControlIndex(2);
pub(super) const WET: ControlIndex = ControlIndex(3);
pub(super) const DRY: ControlIndex = ControlIndex(4);
pub(super) const WIDTH: ControlIndex = ControlIndex(5);
pub(super) const FREEZE: ControlIndex = ControlIndex(6);

/// A number from 0 to 1, clamped there as the reverb applies it.
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
    unit("room_size", 0.5, "Room size"),
    unit("decay", 0.5, "Reverb decay time"),
    unit("damping", 0.5, "High-frequency damping"),
    unit("wet", 0.33, "Wet signal level"),
    unit("dry", 1.0, "Dry signal level"),
    unit("width", 1.0, "Stereo width"),
    ControlDecl::new(
        "freeze",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Infinite hold mode",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every reverb's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
