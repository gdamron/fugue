//! The Oscillator's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const FREQUENCY: ControlIndex = ControlIndex(0);
pub(super) const WAVEFORM: ControlIndex = ControlIndex(1);
pub(super) const FREQUENCY_MOD_DEPTH: ControlIndex = ControlIndex(2);
pub(super) const AMPLITUDE_MOD_DEPTH: ControlIndex = ControlIndex(3);

/// The waveforms, in [`super::OscillatorType`] order.
pub(super) const WAVEFORMS: &[&str] = &["sine", "square", "sawtooth", "triangle"];

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "frequency",
        DeclKind::Number {
            min: 20.0,
            max: 20000.0,
        },
        RtValue::F32(440.0),
        "Frequency in Hz",
    )
    .unit("Hz")
    .clamped(0.0, f32::MAX),
    ControlDecl::new(
        "waveform",
        DeclKind::Choice(WAVEFORMS),
        RtValue::U32(0),
        "Waveform",
    ),
    ControlDecl::new(
        "frequency_mod_depth",
        DeclKind::Number {
            min: 0.0,
            max: 1000.0,
        },
        RtValue::F32(0.0),
        "Frequency modulation depth in Hz",
    )
    .unit("Hz"),
    ControlDecl::new(
        "amplitude_mod_depth",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Amplitude modulation depth, 0 to 1",
    )
    .clamped(0.0, 1.0),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every oscillator's cells
/// start before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
