//! The drive module's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

/// The shapes, by name, in [`crate::dsp::DriveShape::ALL`] order.
pub(super) const SHAPE_NAMES: &[&str] = &["soft", "hard", "fold", "asymmetric", "bits", "rate"];

/// Other names the shapes answer to.
const SHAPE_SPELLINGS: &[(&str, &str)] = &[
    ("soft_clip", "soft"),
    ("saturate", "soft"),
    ("hard_clip", "hard"),
    ("clip", "hard"),
    ("foldback", "fold"),
    ("bit_reduction", "bits"),
    ("bitcrush", "bits"),
    ("rate_reduction", "rate"),
    ("decimate", "rate"),
];

pub(super) const DRIVE: ControlIndex = ControlIndex(0);
pub(super) const SHAPE: ControlIndex = ControlIndex(1);
pub(super) const PRE_TONE: ControlIndex = ControlIndex(2);
pub(super) const POST_TONE: ControlIndex = ControlIndex(3);
pub(super) const BIAS: ControlIndex = ControlIndex(4);
pub(super) const LEVEL: ControlIndex = ControlIndex(5);
pub(super) const AUTO_LEVEL: ControlIndex = ControlIndex(6);
pub(super) const MIX: ControlIndex = ControlIndex(7);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "drive",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.25),
        "How hard the shape is pushed (0 clean to 1 extreme); for bits and rate, how coarse",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "shape",
        DeclKind::Choice(SHAPE_NAMES),
        RtValue::U32(0),
        "The distortion: soft, hard, fold, asymmetric, bits or rate",
    )
    .spellings(SHAPE_SPELLINGS),
    ControlDecl::new(
        "pre_tone",
        DeclKind::Number {
            min: -1.0,
            max: 1.0,
        },
        RtValue::F32(0.0),
        "Tilt before the shape: dark (-1) drives the lows, bright (1) the highs",
    )
    .clamped(-1.0, 1.0),
    ControlDecl::new(
        "post_tone",
        DeclKind::Number {
            min: -1.0,
            max: 1.0,
        },
        RtValue::F32(0.0),
        "Tilt after the shape: dark (-1) tames fizz, bright (1) adds edge",
    )
    .clamped(-1.0, 1.0),
    ControlDecl::new(
        "bias",
        DeclKind::Number {
            min: -1.0,
            max: 1.0,
        },
        RtValue::F32(0.0),
        "Offset into the shape: off-centre adds even harmonics",
    )
    .clamped(-1.0, 1.0),
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Output level (0-1)",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "auto_level",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Keep the level steady as drive changes (set for a -12 dBFS signal)",
    ),
    ControlDecl::new(
        "mix",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Share of the driven signal (0 dry, 1 fully driven)",
    )
    .clamped(0.0, 1.0),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
