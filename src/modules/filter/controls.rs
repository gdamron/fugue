//! The Filter's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const CUTOFF: ControlIndex = ControlIndex(0);
pub(super) const RESONANCE: ControlIndex = ControlIndex(1);
pub(super) const FILTER_TYPE: ControlIndex = ControlIndex(2);
pub(super) const CUTOFF_MOD_DEPTH: ControlIndex = ControlIndex(3);

/// The filter types, in [`super::FilterType`] order.
const FILTER_TYPES: &[&str] = &["lowpass", "highpass", "bandpass"];

/// Other spellings of the filter types that have always loaded, with the
/// option each means.
const FILTER_TYPE_SPELLINGS: &[(&str, &str)] = &[
    ("low_pass", "lowpass"),
    ("lpf", "lowpass"),
    ("low", "lowpass"),
    ("high_pass", "highpass"),
    ("hpf", "highpass"),
    ("high", "highpass"),
    ("band_pass", "bandpass"),
    ("bpf", "bandpass"),
    ("band", "bandpass"),
];

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "cutoff",
        DeclKind::Number {
            min: 20.0,
            max: 20000.0,
        },
        RtValue::F32(1000.0),
        "Cutoff frequency in Hz",
    )
    .unit("Hz")
    .log()
    .precision(0)
    .clamped(20.0, 20000.0),
    ControlDecl::new(
        "resonance",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Resonance/Q",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "filter_type",
        DeclKind::Choice(FILTER_TYPES),
        RtValue::U32(0),
        "Filter type",
    )
    .spellings(FILTER_TYPE_SPELLINGS),
    ControlDecl::new(
        "cutoff_mod_depth",
        DeclKind::Number {
            min: 0.0,
            max: 20000.0,
        },
        RtValue::F32(5000.0),
        "CV modulation depth in Hz",
    )
    .unit("Hz")
    .precision(0)
    .clamped(0.0, f32::MAX),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every filter's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
