//! The contour's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const FREQUENCY: ControlIndex = ControlIndex(0);
pub(super) const SHAPE: ControlIndex = ControlIndex(1);
pub(super) const SLOPE: ControlIndex = ControlIndex(2);
pub(super) const SMOOTHNESS: ControlIndex = ControlIndex(3);
pub(super) const SHIFT: ControlIndex = ControlIndex(4);
pub(super) const MODE: ControlIndex = ControlIndex(5);
pub(super) const RANGE: ControlIndex = ControlIndex(6);
pub(super) const OUTPUT_MODE: ControlIndex = ControlIndex(7);
pub(super) const FREQUENCY_MOD_DEPTH: ControlIndex = ControlIndex(8);
pub(super) const SHAPE_MOD_DEPTH: ControlIndex = ControlIndex(9);
pub(super) const SLOPE_MOD_DEPTH: ControlIndex = ControlIndex(10);
pub(super) const SMOOTHNESS_MOD_DEPTH: ControlIndex = ControlIndex(11);

pub(super) const MIN_FREQUENCY: f32 = 0.001;
pub(super) const MAX_FREQUENCY_HZ: f32 = 20000.0;

pub(super) const MODES: &[&str] = &["ad", "looping", "ar"];
pub(super) const RANGES: &[&str] = &["control", "audio"];
pub(super) const OUTPUT_MODES: &[&str] = &["envelope", "amplitude", "phase", "ratio"];

/// A 0-to-1 control, 0.5 by default.
const fn unit(key: &'static str, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.5),
        description,
    )
    .precision(3)
    .clamped(0.0, 1.0)
}

/// How far a 0-to-1 modulation input moves its control, 1 by default.
const fn depth(key: &'static str, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        description,
    )
    .precision(3)
    .clamped(0.0, 1.0)
}

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "frequency",
        DeclKind::Number {
            min: MIN_FREQUENCY,
            max: MAX_FREQUENCY_HZ,
        },
        RtValue::F32(1.0),
        "Cycles per second; an envelope lasts one cycle. A patched clock sets it instead",
    )
    .unit("Hz")
    .log()
    .precision(3)
    .clamped(MIN_FREQUENCY, MAX_FREQUENCY_HZ),
    unit(
        "shape",
        "Curve of the attack and decay: exponential and logarithmic toward the ends, linear-ish at 0.5",
    ),
    unit(
        "slope",
        "How the cycle splits between attack and decay: short attack at 0, symmetric at 0.5, short decay at 1",
    ),
    unit(
        "smoothness",
        "Below 0.5 smooths the contour toward a sine; above 0.5 folds it for more harmonics",
    ),
    unit(
        "shift",
        "How the four outputs relate in the chosen output mode: level and polarity, pan, phase offset or ratio set",
    ),
    ControlDecl::new(
        "mode",
        DeclKind::Choice(MODES),
        RtValue::U32(1),
        "ad: attack and decay per trigger; looping: cycles; ar: attack while the gate is high, release when it falls",
    ),
    ControlDecl::new(
        "range",
        DeclKind::Choice(RANGES),
        RtValue::U32(0),
        "control: envelope and LFO curves; audio: band-limited oscillator curves",
    ),
    ControlDecl::new(
        "output_mode",
        DeclKind::Choice(OUTPUT_MODES),
        RtValue::U32(2),
        "envelope: shaped, raw and two gates; amplitude: one contour panned across the outputs; phase: phase-offset copies; ratio: copies at related frequencies",
    ),
    ControlDecl::new(
        "frequency_mod_depth",
        DeclKind::Number {
            min: 0.0,
            max: MAX_FREQUENCY_HZ,
        },
        RtValue::F32(1.0),
        "Hz added per unit of the frequency_mod input",
    )
    .unit("Hz")
    .clamped(0.0, MAX_FREQUENCY_HZ),
    depth("shape_mod_depth", "How far the shape_mod input moves shape"),
    depth("slope_mod_depth", "How far the slope_mod input moves slope"),
    depth(
        "smoothness_mod_depth",
        "How far the smoothness_mod input moves smoothness",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
