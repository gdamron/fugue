//! The StepSequencer's controls: its scalars are declared, and its
//! `pattern` stays on the legacy path, shared under a lock, until it is
//! carried as a payload (FUG-312).

use std::sync::{Arc, Mutex};

use crate::control_request::{
    Automation, ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue,
};
use crate::invention::declared::{Declaration, DeclaredSurface, Route};
use crate::traits::ControlSurfaceMap;
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

use super::grace::{DEFAULT_GRACE_DURATION, MAX_GRACE_DURATION, MIN_GRACE_DURATION};
use super::{Step, DEFAULT_GATE_LENGTH, DEFAULT_ROOT_NOTE, DEFAULT_STEPS};

pub(super) const ROOT_NOTE: ControlIndex = ControlIndex(0);
pub(super) const STEP_COUNT: ControlIndex = ControlIndex(1);
pub(super) const GATE_LENGTH: ControlIndex = ControlIndex(2);
pub(super) const MODE: ControlIndex = ControlIndex(3);
pub(super) const GRACE_DURATION: ControlIndex = ControlIndex(4);
pub(super) const GRACE_PLACEMENT: ControlIndex = ControlIndex(5);
pub(super) const ENDED: ControlIndex = ControlIndex(6);

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

/// Where `pattern` sits among the controls a client lists: after
/// `gate_length`, as it always has.
const PATTERN_POSITION: usize = 3;

/// The step sequencer's surface: its declared controls, plus `pattern`.
pub(super) struct StepSequencerSurface {
    pub(super) declared: DeclaredSurface,
    pub(super) pattern: Arc<Mutex<Vec<Step>>>,
}

impl StepSequencerSurface {
    fn pattern_json(&self) -> String {
        let pattern = self.pattern.lock().unwrap().clone();
        serde_json::to_string(&pattern).unwrap_or_else(|_| "[]".to_string())
    }

    fn set_pattern(&self, value: &ControlValue) -> Result<(), String> {
        *self.pattern.lock().unwrap() = parse_pattern_json(value.as_string()?)?;
        Ok(())
    }
}

pub(super) fn parse_pattern_json(value: &str) -> Result<Vec<Step>, String> {
    let pattern: Vec<Step> = serde_json::from_str(value).map_err(|err| err.to_string())?;
    if pattern.len() > MAX_STEPS as usize {
        return Err("pattern may not contain more than 64 steps".to_string());
    }
    Ok(pattern)
}

impl ControlSurface for StepSequencerSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        let mut controls = self.declared.controls();
        let pattern = ControlMeta::string("pattern", "Step pattern as JSON")
            .with_default(self.pattern_json());
        controls.insert(PATTERN_POSITION.min(controls.len()), pattern);
        controls
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "pattern" => Ok(self.pattern_json().into()),
            _ => self.declared.get_control(key),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "pattern" => self.set_pattern(&value),
            _ => self.declared.set_control(key, value),
        }
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        match key {
            "pattern" => parse_pattern_json(value.as_string()?).map(drop),
            _ => self.declared.validate_control(key, value, surfaces),
        }
    }

    #[allow(private_interfaces)]
    fn bind(&self, route: Route, module: &mut dyn Module) {
        self.declared.bind(route, module);
    }

    fn set_legacy(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "pattern" => self.set_pattern(&value),
            _ => self.declared.set_legacy(key, value),
        }
    }

    #[allow(private_interfaces)]
    fn activate(&self, route: Route) {
        self.declared.activate(route);
    }

    fn retire(&self) {
        self.declared.retire();
    }

    fn declares(&self, key: &str) -> bool {
        self.declared.declares(key)
    }

    #[allow(private_interfaces)]
    fn declaration(&self, key: &str) -> Option<Declaration> {
        self.declared.declaration(key)
    }

    #[allow(private_interfaces)]
    fn automation(&self, key: &str) -> Option<Automation> {
        self.declared.automation(key)
    }
}
