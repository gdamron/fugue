//! The delay's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::modules::note_division::{DIVISIONS, DIVISION_SPELLINGS};

pub(super) use crate::modules::note_division::DIVISION_BEATS;

pub(super) const TIME: ControlIndex = ControlIndex(0);
pub(super) const DIVISION: ControlIndex = ControlIndex(1);
pub(super) const BPM: ControlIndex = ControlIndex(2);
pub(super) const FEEDBACK: ControlIndex = ControlIndex(3);
pub(super) const GLIDE: ControlIndex = ControlIndex(4);
pub(super) const HOLD: ControlIndex = ControlIndex(5);
pub(super) const TIME_MOD_DEPTH: ControlIndex = ControlIndex(6);

/// The longest time any delay's controls offer, in seconds.
pub(super) const LONGEST_SECONDS: f32 = 60.0;
/// The most feedback: past 1, repeats grow until the soft clip holds them.
pub(super) const MOST_FEEDBACK: f32 = 1.1;

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
        "hold",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Stop recording and loop the last division",
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
