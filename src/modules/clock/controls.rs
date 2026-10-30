//! The Clock's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const BPM: ControlIndex = ControlIndex(0);
pub(super) const GATE_LENGTH: ControlIndex = ControlIndex(1);
pub(super) const RESET: ControlIndex = ControlIndex(2);
pub(super) const POSITION: ControlIndex = ControlIndex(3);
pub(super) const ELAPSED: ControlIndex = ControlIndex(4);

const DECLS: &[ControlDecl] = &[
    // Not clamped: tests and fast pulse clocks run far past the range
    // editors offer, as they always could.
    ControlDecl::new(
        "bpm",
        DeclKind::Number {
            min: 1.0,
            max: 300.0,
        },
        RtValue::F32(120.0),
        "Tempo in beats per minute",
    )
    .unit("BPM")
    .step(1.0)
    .precision(1),
    ControlDecl::new(
        "gate_length",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.25),
        "Gate length as a fraction of the pulse",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "reset",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Returns the clock to beat 0: its next sample is the first gate again",
    )
    .event(),
    ControlDecl::new(
        "position",
        DeclKind::Number {
            min: 0.0,
            max: f32::MAX,
        },
        RtValue::F32(0.0),
        "Beats since the first gate or the latest reset, as of the latest block",
    )
    .telemetry(),
    ControlDecl::new(
        "elapsed",
        DeclKind::Number {
            min: 0.0,
            max: f32::MAX,
        },
        RtValue::F32(0.0),
        "Beats counted since the clock started, across resets (a reset moves on \
         to the next whole beat), as of the latest block: where a view's playhead is",
    )
    .telemetry(),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every clock's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
