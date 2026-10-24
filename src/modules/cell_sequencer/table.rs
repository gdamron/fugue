//! The CellSequencer's controls, all declared: its scalars, its `cells`
//! bank, a payload built on a control thread and kept whole by the
//! sequencer (the bank it replaces is retired off the audio thread), and
//! the telemetry it publishes about what plays.
//!
//! `select_cell` is a request: it reads back the cell last asked for, and
//! only requests write it. What plays is the `cell` telemetry, which the
//! sequencer moves as a deferred selection lands or one-shot playback
//! advances.

use crate::control_request::{
    ControlDecl, ControlIndex, ControlTable, DeclKind, PayloadCodec, RtValue,
};
use crate::payload::Payload;
use crate::ControlValue;

use super::{
    parse_sequence_bank_json, Step, DEFAULT_GATE_LENGTH, DEFAULT_GRACE_DURATION,
    DEFAULT_GRACE_VELOCITY, DEFAULT_ROOT_NOTE, DEFAULT_STEPS, MAX_GRACE_DURATION, MAX_SEQUENCES,
    MAX_STEPS, MIN_GRACE_DURATION,
};

pub(super) const ROOT_NOTE: ControlIndex = ControlIndex(0);
pub(super) const STEP_COUNT: ControlIndex = ControlIndex(1);
pub(super) const FOLLOW_CELL_LENGTH: ControlIndex = ControlIndex(2);
pub(super) const GATE_LENGTH: ControlIndex = ControlIndex(3);
pub(super) const SELECT_CELL: ControlIndex = ControlIndex(4);
pub(super) const WAIT_FOR_CYCLE_END: ControlIndex = ControlIndex(5);
pub(super) const CELLS: ControlIndex = ControlIndex(6);
pub(super) const MODE: ControlIndex = ControlIndex(7);
pub(super) const LOOP_COUNT: ControlIndex = ControlIndex(8);
pub(super) const CELL: ControlIndex = ControlIndex(9);
pub(super) const CELL_COUNT: ControlIndex = ControlIndex(10);
pub(super) const NEXT_CELL: ControlIndex = ControlIndex(11);
pub(super) const GRACE_DURATION: ControlIndex = ControlIndex(12);
pub(super) const GRACE_VELOCITY: ControlIndex = ControlIndex(13);
pub(super) const GRACE_PLACEMENT: ControlIndex = ControlIndex(14);
pub(super) const ENDED: ControlIndex = ControlIndex(15);

const LAST_CELL: i32 = MAX_SEQUENCES as i32 - 1;

/// The most loops `loop_count` reports exactly through a client's `f32`.
const MAX_LOOPS: i32 = 1 << 24;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "root_note",
        DeclKind::Integer { min: 0, max: 127 },
        RtValue::I32(DEFAULT_ROOT_NOTE as i32),
        "Root MIDI note",
    )
    .clamped(0.0, 127.0),
    ControlDecl::new(
        "step_count",
        DeclKind::Integer {
            min: 1,
            max: MAX_STEPS as i32,
        },
        RtValue::I32(DEFAULT_STEPS as i32),
        "Number of steps per cell",
    )
    .clamped(1.0, MAX_STEPS as f32),
    ControlDecl::new(
        "follow_cell_length",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Wrap each cycle at the selected cell's own length instead of steps",
    ),
    ControlDecl::new(
        "gate_length",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(DEFAULT_GATE_LENGTH),
        "Default gate length ratio",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "select_cell",
        DeclKind::Integer {
            min: 0,
            max: LAST_CELL,
        },
        RtValue::I32(0),
        "Cell to play: reads back the cell last asked for (the cell playing is `cell`)",
    )
    .clamped(0.0, LAST_CELL as f32),
    ControlDecl::new(
        "wait_for_cycle_end",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Defer cell changes until the current cycle ends",
    ),
    ControlDecl::payload("cells", BankCodec::CODEC, "Cells as JSON"),
    ControlDecl::new(
        "mode",
        DeclKind::Choice(&["loop", "one_shot"]),
        RtValue::U32(0),
        "Playback mode: loop repeats the active cell; one_shot plays the bank through once and fires the end gate",
    ),
    ControlDecl::new(
        "loop_count",
        DeclKind::Integer {
            min: 0,
            max: MAX_LOOPS,
        },
        RtValue::I32(0),
        "Completed loops of the active cell",
    )
    .telemetry(),
    ControlDecl::new(
        "cell",
        DeclKind::Integer {
            min: 0,
            max: LAST_CELL,
        },
        RtValue::I32(0),
        "Currently playing cell index",
    )
    .telemetry(),
    ControlDecl::new(
        "cell_count",
        DeclKind::Integer {
            min: 0,
            max: MAX_SEQUENCES as i32,
        },
        RtValue::I32(0),
        "Total number of cells in the bank",
    )
    .telemetry(),
    ControlDecl::new(
        "next_cell",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Trigger: a write above 0.5 advances to the next cell",
    )
    .event(),
    ControlDecl::new(
        "grace_duration",
        DeclKind::Number {
            min: MIN_GRACE_DURATION,
            max: MAX_GRACE_DURATION,
        },
        RtValue::F32(DEFAULT_GRACE_DURATION),
        "Duration of a single grace note in seconds",
    )
    .unit("s")
    .clamped(MIN_GRACE_DURATION, MAX_GRACE_DURATION),
    ControlDecl::new(
        "grace_velocity",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(DEFAULT_GRACE_VELOCITY),
        "Velocity scale for grace notes relative to the decorated step",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "grace_placement",
        DeclKind::Choice(&["before", "on_beat"]),
        RtValue::U32(0),
        "Grace placement: before steals the previous step's tail (principal stays on the grid); on_beat starts the chain at the step edge and delays the principal",
    ),
    ControlDecl::new(
        "ended",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Read-only: a one_shot bank playthrough has completed",
    )
    .telemetry(),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every cell sequencer's
/// cells start before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}

/// A bank of cells, as the sequencer keeps it.
pub(super) type Bank = Vec<Vec<Step>>;

/// Builds a written `cells` bank into the payload the sequencer keeps.
struct BankCodec;

impl BankCodec {
    const CODEC: PayloadCodec = PayloadCodec {
        prepare: Self::prepare,
    };

    fn prepare(value: &ControlValue) -> Result<(Payload, ControlValue), String> {
        let bank = parse_sequence_bank_json(value.as_string()?)?;
        let shown = bank_json(&bank).into();
        Ok((Payload::new(bank), shown))
    }
}

/// `cells` as the control reads back: JSON text.
pub(super) fn bank_json(bank: &[Vec<Step>]) -> String {
    serde_json::to_string(bank).unwrap_or_else(|_| "[]".to_string())
}
