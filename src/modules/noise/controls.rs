//! The noise module's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const LEVEL: ControlIndex = ControlIndex(0);

const DECLS: &[ControlDecl] = &[ControlDecl::new(
    "level",
    DeclKind::Number { min: 0.0, max: 1.0 },
    RtValue::F32(1.0),
    "Output level of both noise colours (0-1)",
)
.clamped(0.0, 1.0)];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
