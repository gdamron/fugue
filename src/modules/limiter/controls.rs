//! The limiter's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const CEILING: ControlIndex = ControlIndex(0);
pub(super) const RELEASE: ControlIndex = ControlIndex(1);
pub(super) const REDUCTION: ControlIndex = ControlIndex(2);

/// The ceiling's range and default, in dB.
pub(super) const MIN_CEILING: f32 = -24.0;
pub(super) const MAX_CEILING: f32 = 0.0;
pub(super) const DEFAULT_CEILING: f32 = -1.0;
/// The release's range and default, in seconds.
pub(super) const MIN_RELEASE: f32 = 0.001;
pub(super) const MAX_RELEASE: f32 = 2.0;
pub(super) const DEFAULT_RELEASE: f32 = 0.1;
/// The most reduction `reduction` reports, in dB.
pub(super) const MAX_REDUCTION: f32 = 96.0;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "ceiling",
        DeclKind::Number {
            min: MIN_CEILING,
            max: MAX_CEILING,
        },
        RtValue::F32(DEFAULT_CEILING),
        "The level no output sample exceeds, in dBFS",
    )
    .unit("dB")
    .clamped(MIN_CEILING, MAX_CEILING),
    ControlDecl::new(
        "release",
        DeclKind::Number {
            min: MIN_RELEASE,
            max: MAX_RELEASE,
        },
        RtValue::F32(DEFAULT_RELEASE),
        "How quickly the level recovers after a peak (time constant)",
    )
    .unit("s")
    .log()
    .clamped(MIN_RELEASE, MAX_RELEASE),
    ControlDecl::new(
        "reduction",
        DeclKind::Number {
            min: 0.0,
            max: MAX_REDUCTION,
        },
        RtValue::F32(0.0),
        "Read-only: how many dB the limiter pulled down in the last block",
    )
    .unit("dB")
    .telemetry(),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
