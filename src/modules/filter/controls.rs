//! The Filter's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::ControlWidget;

pub(super) const CUTOFF: ControlIndex = ControlIndex(0);
pub(super) const RESONANCE: ControlIndex = ControlIndex(1);
pub(super) const FILTER_TYPE: ControlIndex = ControlIndex(2);
pub(super) const CUTOFF_MOD_DEPTH: ControlIndex = ControlIndex(3);
pub(super) const BOOST: ControlIndex = ControlIndex(4);
pub(super) const Q: ControlIndex = ControlIndex(5);
pub(super) const BYPASS: ControlIndex = ControlIndex(6);
pub(super) const SLOPE: ControlIndex = ControlIndex(7);

/// The `boost` range, in dB.
pub(super) const MAX_BOOST: f32 = 24.0;
/// The `q` range of the EQ responses.
pub(super) const MIN_Q: f32 = 0.1;
pub(super) const MAX_Q: f32 = 18.0;
/// The `q` an EQ response starts with: a shelf without overshoot.
pub(super) const DEFAULT_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The filter types, in [`super::FilterType`] order.
const FILTER_TYPES: &[&str] = &[
    "lowpass",
    "highpass",
    "bandpass",
    "bell",
    "lowshelf",
    "highshelf",
    "notch",
];

/// The filter types as a person reads them, where tidying says them badly.
const FILTER_TYPE_LABELS: &[(&str, &str)] = &[
    ("lowpass", "Low-pass"),
    ("highpass", "High-pass"),
    ("bandpass", "Band-pass"),
    ("lowshelf", "Low shelf"),
    ("highshelf", "High shelf"),
];

/// The slopes of the lowpass, highpass and bandpass responses.
const SLOPES: &[&str] = &["12db", "24db"];

/// The slopes as a person reads them.
const SLOPE_LABELS: &[(&str, &str)] = &[("12db", "12 dB/oct"), ("24db", "24 dB/oct")];

/// Other spellings of the slopes.
const SLOPE_SPELLINGS: &[(&str, &str)] = &[("12", "12db"), ("24", "24db")];

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
    ("peak", "bell"),
    ("peaking", "bell"),
    ("low_shelf", "lowshelf"),
    ("high_shelf", "highshelf"),
    ("bandstop", "notch"),
    ("band_stop", "notch"),
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
    .clamped(20.0, 20000.0)
    .widget(ControlWidget::Knob),
    ControlDecl::new(
        "resonance",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Resonance/Q",
    )
    .clamped(0.0, 1.0)
    .widget(ControlWidget::Knob),
    ControlDecl::new(
        "filter_type",
        DeclKind::Choice(FILTER_TYPES),
        RtValue::U32(0),
        "Filter type",
    )
    .spellings(FILTER_TYPE_SPELLINGS)
    .option_labels(FILTER_TYPE_LABELS)
    .widget(ControlWidget::Menu),
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
    .clamped(0.0, f32::MAX)
    .widget(ControlWidget::Knob),
    ControlDecl::new(
        "boost",
        DeclKind::Number {
            min: -MAX_BOOST,
            max: MAX_BOOST,
        },
        RtValue::F32(0.0),
        "Gain at the band in dB for bell and shelves; negative cuts",
    )
    .unit("dB")
    .precision(1)
    .clamped(-MAX_BOOST, MAX_BOOST)
    .widget(ControlWidget::Knob),
    ControlDecl::new(
        "q",
        DeclKind::Number {
            min: MIN_Q,
            max: MAX_Q,
        },
        RtValue::F32(DEFAULT_Q),
        "Width of a bell or notch; steepness of a shelf (which stops at 2)",
    )
    .log()
    .precision(2)
    .clamped(MIN_Q, MAX_Q)
    .widget(ControlWidget::Knob),
    ControlDecl::new(
        "bypass",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Pass the input through unfiltered (crossfades)",
    ),
    ControlDecl::new(
        "slope",
        DeclKind::Choice(SLOPES),
        RtValue::U32(0),
        "Lowpass, highpass and bandpass slope: 12 or 24 dB per octave",
    )
    .spellings(SLOPE_SPELLINGS)
    .option_labels(SLOPE_LABELS)
    .widget(ControlWidget::Segmented),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every filter's cells start
/// before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
