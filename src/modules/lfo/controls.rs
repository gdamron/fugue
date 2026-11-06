//! The Lfo's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::modules::note_division::{DIVISIONS, DIVISION_SPELLINGS};
use crate::modules::oscillator::{WAVEFORMS, WAVEFORM_SPELLINGS};

pub(super) const RATE: ControlIndex = ControlIndex(0);
pub(super) const RATE_MOD_DEPTH: ControlIndex = ControlIndex(1);
pub(super) const WAVEFORM: ControlIndex = ControlIndex(2);
pub(super) const DIVISION: ControlIndex = ControlIndex(3);
pub(super) const BPM: ControlIndex = ControlIndex(4);
pub(super) const PHASE: ControlIndex = ControlIndex(5);

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
    ControlDecl::new(
        "division",
        DeclKind::Choice(DIVISIONS),
        RtValue::U32(0),
        "One cycle as a note value at the tempo; off uses rate",
    )
    .spellings(DIVISION_SPELLINGS),
    ControlDecl::new(
        "bpm",
        DeclKind::Number {
            min: 1.0,
            max: 1000.0,
        },
        RtValue::F32(120.0),
        "Tempo that times division, unless the bpm input is patched",
    )
    .unit("BPM")
    .precision(1)
    .clamped(1.0, 1000.0),
    ControlDecl::new(
        "phase",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Offset added to the phase, in cycles; changes glide",
    )
    .precision(3)
    .clamped(0.0, 1.0),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every LFO's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
