//! The phaser's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::modules::note_division::{DIVISIONS, DIVISION_SPELLINGS};

pub(super) const STAGE_COUNT: ControlIndex = ControlIndex(0);
pub(super) const RATE: ControlIndex = ControlIndex(1);
pub(super) const DIVISION: ControlIndex = ControlIndex(2);
pub(super) const BPM: ControlIndex = ControlIndex(3);
pub(super) const DEPTH: ControlIndex = ControlIndex(4);
pub(super) const FREQUENCY: ControlIndex = ControlIndex(5);
pub(super) const FEEDBACK: ControlIndex = ControlIndex(6);
pub(super) const STEREO_PHASE: ControlIndex = ControlIndex(7);
pub(super) const MIX: ControlIndex = ControlIndex(8);

/// The fewest and most allpass stages per side.
pub(super) const FEWEST_STAGES: i32 = 4;
pub(super) const MOST_STAGES: usize = 12;
/// The most feedback either way: the loop stays well clear of unity.
pub(super) const MOST_FEEDBACK: f32 = 0.95;

const fn number(
    key: &'static str,
    min: f32,
    max: f32,
    default: f32,
    description: &'static str,
) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min, max },
        RtValue::F32(default),
        description,
    )
    .clamped(min, max)
}

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "stage_count",
        DeclKind::Integer {
            min: FEWEST_STAGES,
            max: MOST_STAGES as i32,
        },
        RtValue::I32(6),
        "Allpass stages per side (4 to 12): more stages, more notches. A change crossfades",
    )
    .clamped(FEWEST_STAGES as f32, MOST_STAGES as f32),
    number("rate", 0.01, 10.0, 0.5, "Sweep speed while division is off")
        .unit("Hz")
        .log()
        .precision(3),
    ControlDecl::new(
        "division",
        DeclKind::Choice(DIVISIONS),
        RtValue::U32(0),
        "One sweep as a note value at the tempo; off uses rate",
    )
    .spellings(DIVISION_SPELLINGS),
    number(
        "bpm",
        1.0,
        1000.0,
        120.0,
        "Tempo that times division, unless the bpm input is patched",
    )
    .unit("BPM")
    .precision(1),
    number(
        "depth",
        0.0,
        1.0,
        1.0,
        "Sweep width: up to 3 octaves either side of frequency",
    ),
    number("frequency", 50.0, 4000.0, 1000.0, "Centre of the sweep")
        .unit("Hz")
        .log()
        .precision(0),
    number(
        "feedback",
        -MOST_FEEDBACK,
        MOST_FEEDBACK,
        0.4,
        "Share of the stages' output fed back to their input; negative swaps notches and peaks",
    ),
    number(
        "stereo_phase",
        0.0,
        1.0,
        0.1,
        "How far the right sweep runs ahead of the left, in cycles",
    ),
    number(
        "mix",
        0.0,
        1.0,
        0.5,
        "0 is the dry input, 0.5 the classic notches, 1 the stages alone",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
