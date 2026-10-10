//! The StepSequencer's controls, all declared: its scalars, and its
//! `pattern`, a payload built on a control thread and kept whole by the
//! sequencer; the pattern it replaces is retired off the audio thread.

use crate::control_request::{
    ControlDecl, ControlIndex, ControlTable, DeclKind, PayloadCodec, RtValue,
};
use crate::payload::Payload;
use crate::ControlValue;

use super::grace::{DEFAULT_GRACE_DURATION, MAX_GRACE_DURATION, MIN_GRACE_DURATION};
use super::{Step, DEFAULT_GATE_LENGTH, DEFAULT_ROOT_NOTE, DEFAULT_STEPS};

pub(super) const ROOT_NOTE: ControlIndex = ControlIndex(0);
pub(super) const STEP_COUNT: ControlIndex = ControlIndex(1);
pub(super) const GATE_LENGTH: ControlIndex = ControlIndex(2);
pub(super) const PATTERN: ControlIndex = ControlIndex(3);
pub(super) const MODE: ControlIndex = ControlIndex(4);
pub(super) const GRACE_DURATION: ControlIndex = ControlIndex(5);
pub(super) const GRACE_PLACEMENT: ControlIndex = ControlIndex(6);
pub(super) const ENDED: ControlIndex = ControlIndex(7);

/// The most steps a pattern holds, and so the most `step_count` takes.
pub(super) const MAX_STEPS: i32 = 64;

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
            max: MAX_STEPS,
        },
        RtValue::I32(DEFAULT_STEPS as i32),
        "Number of steps in pattern",
    )
    .clamped(1.0, MAX_STEPS as f32),
    ControlDecl::new(
        "gate_length",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(DEFAULT_GATE_LENGTH),
        "Default gate length ratio",
    )
    .clamped(0.0, 1.0),
    ControlDecl::payload("pattern", PatternCodec::CODEC, "Step pattern as JSON"),
    ControlDecl::new(
        "mode",
        DeclKind::Choice(&["loop", "one_shot"]),
        RtValue::U32(0),
        "Playback mode: loop repeats; one_shot plays once and fires the ended gate",
    ),
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
        "grace_placement",
        DeclKind::Choice(&["before", "on_beat"]),
        RtValue::U32(0),
        "Grace placement: before steals the previous step's tail; on_beat delays the principal",
    ),
    ControlDecl::new(
        "ended",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Read-only: a one_shot pattern has played through",
    )
    .telemetry(),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every step sequencer's cells
/// start before it applies its own values.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}

/// Builds a written `pattern` into the payload the sequencer keeps.
struct PatternCodec;

impl PatternCodec {
    const CODEC: PayloadCodec = PayloadCodec {
        prepare: Self::prepare,
    };

    fn prepare(value: &ControlValue) -> Result<(Payload, ControlValue), String> {
        let pattern = parse_pattern_json(value.as_string()?)?;
        let shown = pattern_json(&pattern).into();
        Ok((Payload::new(pattern), shown))
    }
}

/// `pattern` as the control reads back: JSON text.
pub(super) fn pattern_json(pattern: &[Step]) -> String {
    serde_json::to_string(pattern).unwrap_or_else(|_| "[]".to_string())
}

pub(super) fn parse_pattern_json(value: &str) -> Result<Vec<Step>, String> {
    let pattern: Vec<Step> = serde_json::from_str(value).map_err(|err| err.to_string())?;
    if pattern.len() > MAX_STEPS as usize {
        return Err("pattern may not contain more than 64 steps".to_string());
    }
    Ok(pattern)
}
