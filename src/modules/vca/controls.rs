//! The Vca's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const LEVEL: ControlIndex = ControlIndex(0);

const DECLS: &[ControlDecl] = &[ControlDecl::new(
    "level",
    DeclKind::Number { min: 0.0, max: 1.0 },
    RtValue::F32(1.0),
    "Default level (when no signal connected)",
)
.clamped(0.0, 1.0)];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);
