//! A module's automation at one sample: its values apply before its
//! events, whichever control comes first and whichever was written first.
//! Across samples (taken late), each in its sample's order.

use std::sync::Arc;

use super::super::{ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use super::take_automation;
use crate::control_request::hold::FrameScope;
use crate::control_request::Refusal;
use crate::Module;

const FIRE: ControlIndex = ControlIndex(0);
const LEVEL: ControlIndex = ControlIndex(1);
const NOTE: ControlIndex = ControlIndex(2);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new("fire", DeclKind::Bool, RtValue::Bool(false), "An event").event(),
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "A value",
    ),
    ControlDecl::new(
        "note",
        DeclKind::Number {
            min: 0.0,
            max: 127.0,
        },
        RtValue::F32(0.0),
        "A queued event",
    )
    .event(),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

/// Records each control as it applies.
struct Recorder {
    cells: Arc<ControlCells>,
    applied: Vec<u16>,
}

impl Module for Recorder {
    fn name(&self) -> &str {
        "Recorder"
    }
    fn process(&mut self, _frames: usize) -> bool {
        true
    }
    fn inputs(&self) -> &[&str] {
        &[]
    }
    fn outputs(&self) -> &[&str] {
        &[]
    }
    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }
    fn output_block(&self, _index: usize) -> &[f32] {
        &[]
    }
    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input '{port}'"))
    }
    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output '{port}'"))
    }
    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        self.applied.push(control.0);
        Ok(value)
    }
    fn finish_events(&mut self) {
        self.applied.push(FINISHED);
    }
}

/// Recorded as the events applied so far finish.
const FINISHED: u16 = u16::MAX;

fn recorder(cells: ControlCells) -> Recorder {
    Recorder {
        cells: Arc::new(cells),
        applied: Vec::new(),
    }
}

#[test]
fn an_event_slot_applies_after_the_values_written_beside_it() {
    let mut module = recorder(ControlCells::new(DECLS.iter().map(|d| d.default)));
    let slots = &module.cells.automation;
    slots.write(FIRE, RtValue::Bool(true), true);
    slots.write(LEVEL, RtValue::F32(0.5), false);
    take_automation(&mut module);
    assert_eq!(module.applied, [LEVEL.0, FIRE.0]);
}

#[test]
fn queued_events_apply_after_the_values_written_beside_them() {
    let cells = ControlCells::queuing_events(DECLS.iter().map(|d| d.default), 8, None);
    let mut module = recorder(cells);
    let slots = &module.cells.automation;
    slots.write(NOTE, RtValue::F32(60.0), true);
    slots.write(FIRE, RtValue::Bool(true), true);
    slots.write(LEVEL, RtValue::F32(0.5), false);
    slots.write(NOTE, RtValue::F32(64.0), true);
    take_automation(&mut module);
    assert_eq!(module.applied, [LEVEL.0, NOTE.0, FIRE.0, NOTE.0]);
}

/// Taken late, writes span samples: a value written for a later sample
/// than a queued event applies after it, and before the events written
/// for its own sample, whichever was written first there.
#[test]
fn a_value_written_after_a_queued_events_sample_applies_after_it() {
    let cells = ControlCells::queuing_events(DECLS.iter().map(|d| d.default), 8, None);
    let mut module = recorder(cells);
    let slots = &module.cells.automation;
    let at = |sample| FrameScope::enter(sample);
    {
        let _at = at(10);
        slots.write(LEVEL, RtValue::F32(0.25), false);
        slots.write(NOTE, RtValue::F32(60.0), true);
    }
    {
        let _at = at(12);
        slots.write(NOTE, RtValue::F32(64.0), true);
        slots.write(LEVEL, RtValue::F32(0.5), false);
    }
    take_automation(&mut module);
    let applied = [LEVEL.0, NOTE.0, FINISHED, LEVEL.0, NOTE.0];
    assert_eq!(module.applied, applied, "sample 10's note finished first");
    assert_eq!(module.cells.load(LEVEL), Some(RtValue::F32(0.5)));
}

/// A value queued behind an event is the latest written, as a ramp
/// starting from it reads.
#[test]
fn a_value_queued_behind_an_event_is_the_latest_written() {
    let cells = ControlCells::queuing_events(DECLS.iter().map(|d| d.default), 8, None);
    let module = recorder(cells);
    let slots = &module.cells.automation;
    {
        let _at = FrameScope::enter(10);
        slots.write(LEVEL, RtValue::F32(0.25), false);
        slots.write(NOTE, RtValue::F32(60.0), true);
    }
    let _at = FrameScope::enter(12);
    slots.write(LEVEL, RtValue::F32(0.5), false);
    assert_eq!(slots.written(LEVEL), Some(RtValue::F32(0.5)));
}

/// With no room behind the events, the value still applies, in its slot.
#[test]
fn a_value_finding_no_room_behind_the_events_is_kept_in_its_slot() {
    let cells = ControlCells::queuing_events(DECLS.iter().map(|d| d.default), 1, None);
    let mut module = recorder(cells);
    let slots = &module.cells.automation;
    {
        let _at = FrameScope::enter(10);
        slots.write(NOTE, RtValue::F32(60.0), true);
    }
    let _at = FrameScope::enter(12);
    slots.write(LEVEL, RtValue::F32(0.5), false);
    drop(_at);
    take_automation(&mut module);
    assert_eq!(module.applied, [LEVEL.0, NOTE.0]);
}

/// A value finding no room behind the events goes in its slot, and one
/// queued before it for its control is dropped: the latest write stands.
#[test]
fn a_value_put_in_its_slot_for_want_of_room_stays_the_latest() {
    let cells = ControlCells::queuing_events(DECLS.iter().map(|d| d.default), 2, None);
    let mut module = recorder(cells);
    let slots = &module.cells.automation;
    for (sample, write) in [(10, None), (11, Some(0.25)), (12, Some(0.5))] {
        let _at = FrameScope::enter(sample);
        match write {
            None => slots.write(NOTE, RtValue::F32(60.0), true),
            Some(level) => slots.write(LEVEL, RtValue::F32(level), false),
        }
    }
    assert_eq!(slots.written(LEVEL), Some(RtValue::F32(0.5)));
    take_automation(&mut module);
    assert_eq!(module.applied, [LEVEL.0, NOTE.0]);
    assert_eq!(module.cells.load(LEVEL), Some(RtValue::F32(0.5)));
}
