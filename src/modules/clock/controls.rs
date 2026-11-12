//! The Clock's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const BPM: ControlIndex = ControlIndex(0);
pub(super) const GATE_LENGTH: ControlIndex = ControlIndex(1);
pub(super) const RESET: ControlIndex = ControlIndex(2);
pub(super) const POSITION: ControlIndex = ControlIndex(3);
pub(super) const ELAPSED: ControlIndex = ControlIndex(4);
pub(super) const BEATS_PER_BAR: ControlIndex = ControlIndex(5);
pub(super) const BAR: ControlIndex = ControlIndex(6);
pub(super) const BEAT_IN_BAR: ControlIndex = ControlIndex(7);
pub(super) const METER: ControlIndex = ControlIndex(8);

/// The most beats a bar may have.
pub(super) const MAX_BEATS_PER_BAR: i32 = 32;
/// The highest bar the clock reports (an integer control holds whole
/// numbers within 2^24): past it, it stays on this bar.
pub(super) const MAX_BAR: i32 = 1 << 24;

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
    ControlDecl::new(
        "beats_per_bar",
        DeclKind::Integer {
            min: 0,
            max: MAX_BEATS_PER_BAR,
        },
        RtValue::I32(0),
        "Beats in a bar, 0 for no meter. A new meter takes over at the next \
         bar line; on a clock without one, at once",
    )
    .clamped(0.0, MAX_BEATS_PER_BAR as f32),
    ControlDecl::new(
        "bar",
        DeclKind::Integer {
            min: 0,
            max: MAX_BAR,
        },
        RtValue::I32(0),
        "The bar, from 1 at the latest reset, as of the latest block; 0 with no meter",
    )
    .telemetry(),
    ControlDecl::new(
        "beat_in_bar",
        DeclKind::Integer {
            min: 0,
            max: MAX_BEATS_PER_BAR,
        },
        RtValue::I32(0),
        "The beat within the bar, from 1, as of the latest block; 0 with no meter",
    )
    .telemetry(),
    ControlDecl::new(
        "meter",
        DeclKind::Integer {
            min: 0,
            max: MAX_BEATS_PER_BAR,
        },
        RtValue::I32(0),
        "Beats in the bar playing now: the meter in force, 0 for none",
    )
    .telemetry(),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every clock's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
