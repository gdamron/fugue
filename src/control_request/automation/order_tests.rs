//! A module's automation at one sample: its values apply before its
//! events, whichever control comes first and whichever was written first.

use std::sync::Arc;

use super::super::{ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use super::take_automation;
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
}

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
