//! The delay's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const TIME: ControlIndex = ControlIndex(0);
pub(super) const DIVISION: ControlIndex = ControlIndex(1);
pub(super) const BPM: ControlIndex = ControlIndex(2);
pub(super) const FEEDBACK: ControlIndex = ControlIndex(3);
pub(super) const GLIDE: ControlIndex = ControlIndex(4);
pub(super) const TIME_MOD_DEPTH: ControlIndex = ControlIndex(5);

/// The longest time any delay's controls offer, in seconds.
pub(super) const LONGEST_SECONDS: f32 = 60.0;
/// The most feedback: past 1, repeats grow until the soft clip holds them.
pub(super) const MOST_FEEDBACK: f32 = 1.1;

/// The note values `division` offers, `off` first, in the music module's
/// vocabulary (`8th_note`, `quarter_triplet`).
const DIVISIONS: &[&str] = &[
    "off",
    "whole_note",
    "half_note",
    "quarter_note",
    "8th_note",
    "16th_note",
    "32nd_note",
    "dotted_half_note",
    "dotted_quarter_note",
    "dotted_8th_note",
    "dotted_16th_note",
    "half_triplet",
    "quarter_triplet",
    "8th_triplet",
    "16th_triplet",
];

/// Each division's length in beats (a quarter note is one), in
/// [`DIVISIONS`] order; `off` has none.
pub(super) const DIVISION_BEATS: [f64; 15] = [
    0.0,
    4.0,
    2.0,
    1.0,
    0.5,
    0.25,
    0.125,
    3.0,
    1.5,
    0.75,
    0.375,
    4.0 / 3.0,
    2.0 / 3.0,
    1.0 / 3.0,
    1.0 / 6.0,
];

/// The fractions musicians write, as other spellings of the divisions.
const DIVISION_SPELLINGS: &[(&str, &str)] = &[
    ("1/1", "whole_note"),
    ("1/2", "half_note"),
    ("1/4", "quarter_note"),
    ("1/8", "8th_note"),
    ("1/16", "16th_note"),
    ("1/32", "32nd_note"),
    ("1/2.", "dotted_half_note"),
    ("1/4.", "dotted_quarter_note"),
    ("1/8.", "dotted_8th_note"),
    ("1/16.", "dotted_16th_note"),
    ("1/2t", "half_triplet"),
    ("1/4t", "quarter_triplet"),
    ("1/8t", "8th_triplet"),
    ("1/16t", "16th_triplet"),
];

/// How a change of time is heard, in [`super::Glide`] order.
const GLIDES: &[&str] = &["crossfade", "slew"];

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "time",
        DeclKind::Number {
            min: 0.0,
            max: LONGEST_SECONDS,
        },
        RtValue::F32(0.25),
        "Delay time in seconds, while division is off (up to the configured max_time)",
    )
    .unit("s")
    .precision(3)
    .clamped(0.0, LONGEST_SECONDS),
    ControlDecl::new(
        "division",
        DeclKind::Choice(DIVISIONS),
        RtValue::U32(0),
        "Delay time as a note value at the tempo; off uses time",
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
        "feedback",
        DeclKind::Number {
            min: 0.0,
            max: MOST_FEEDBACK,
        },
        RtValue::F32(0.0),
        "Share of the output fed back into the line; past 1 it self-oscillates, held by a soft clip",
    )
    .clamped(0.0, MOST_FEEDBACK),
    ControlDecl::new(
        "glide",
        DeclKind::Choice(GLIDES),
        RtValue::U32(0),
        "How a time change is heard: crossfade between read heads, or slew the head like tape",
    ),
    ControlDecl::new(
        "time_mod_depth",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.005),
        "Seconds of delay per unit of the time_mod input",
    )
    .unit("s")
    .precision(4)
    .clamped(0.0, 1.0),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
