//! The Lfo's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::modules::oscillator::{WAVEFORMS, WAVEFORM_SPELLINGS};

pub(super) const RATE: ControlIndex = ControlIndex(0);
pub(super) const RATE_MOD_DEPTH: ControlIndex = ControlIndex(1);
pub(super) const WAVEFORM: ControlIndex = ControlIndex(2);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "rate",
        DeclKind::Number {
            min: 0.001,
            max: 100.0,
        },
        RtValue::F32(1.0),
        "LFO rate in Hz",
    )
    .unit("Hz")
    .log()
    .precision(3)
    .clamped(0.001, 100.0),
    ControlDecl::new(
        "rate_mod_depth",
        DeclKind::Number {
            min: 0.0,
            max: 100.0,
        },
        RtValue::F32(1.0),
        "Rate modulation depth in Hz",
    )
    .unit("Hz")
    .clamped(0.0, f32::MAX),
    ControlDecl::new(
        "waveform",
        DeclKind::Choice(WAVEFORMS),
        RtValue::U32(0),
        "Waveform type",
    )
    .spellings(WAVEFORM_SPELLINGS),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every LFO's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
