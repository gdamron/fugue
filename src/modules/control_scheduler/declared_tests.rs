//! A scheduler writing a module that declares its controls: through the
//! target's automation slots, applied when the target is about to process,
//! never locking, allocating or formatting, even when every write fails.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::schedule::{parse_schedule_json, SurfaceMap};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{
    take_automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, EventCursor,
    Refusal, RtValue,
};
use crate::invention::declared::DeclaredSurface;
use crate::ControlSurface;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Level",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new("tap", DeclKind::Bool, RtValue::Bool(false), "Tap").event(),
    ControlDecl::new(
        "voices",
        DeclKind::Integer { min: 0, max: 8 },
        RtValue::I32(0),
        "Voices",
    ),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

/// Holds a level and a voice count of at most 4 (it refuses more).
struct Knob {
    level: f32,
    voices: i32,
    cells: Arc<ControlCells>,
}

impl Module for Knob {
    fn name(&self) -> &str {
        "Knob"
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
        unreachable!()
    }
    fn output_block(&self, _index: usize) -> &[f32] {
        unreachable!()
    }
    fn set_input(&mut self, _port: &str, _value: f32) -> Result<(), String> {
        Err("no inputs".into())
    }
    fn get_output(&self, _port: &str) -> Result<f32, String> {
        Err("no outputs".into())
    }
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control.0, value) {
            (0, RtValue::F32(level)) => self.level = level.clamp(0.0, 1.0),
            (2, RtValue::I32(voices)) if voices <= 4 => self.voices = voices,
            (2, RtValue::I32(_)) => return Err(Refusal::Invalid),
            _ => return Err(Refusal::Unsupported),
        }
        Ok(value)
    }
}

/// A scheduler attached to a directory holding one knob (id `knob`).
fn setup(
    schedule_json: &str,
) -> (
    ControlScheduler,
    Knob,
    Arc<dyn ControlSurface + Send + Sync>,
) {
    let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
    let surface: Arc<dyn ControlSurface + Send + Sync> =
        Arc::new(DeclaredSurface::new(TABLE.clone(), cells.clone()));
    let mut map: SurfaceMap = IndexMap::new();
    map.insert("knob".to_string(), surface.clone());
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));
    let ctrl = ControlSchedulerControls::new(parse_schedule_json(schedule_json).unwrap());
    ctrl.attach("sched", &directory).unwrap();
    let mut scheduler = ControlScheduler::new(48_000, ctrl);
    scheduler.prepare_for_publication();
    let knob = Knob {
        level: 1.0,
        voices: 0,
        cells,
    };
    (scheduler, knob, surface)
}

/// One gate edge and `frames - 1` low frames, then the knob's turn to
/// process: as the graph runs a scheduler ahead of its target.
fn step(scheduler: &mut ControlScheduler, knob: &mut Knob, frames: usize) {
    scheduler.set_input("clock", 1.0).unwrap();
    scheduler.process(1);
    scheduler.set_input("clock", 0.0).unwrap();
    scheduler.process(frames - 1);
    take_automation(knob);
    knob.process(frames);
}

#[test]
fn a_scheduled_write_waits_for_its_target_to_process_then_reads_back() {
    let (mut scheduler, mut knob, surface) =
        setup(r#"[{ "at_step": 0, "module": "knob", "control": "level", "value": 0.5 }]"#);
    scheduler.set_input("clock", 1.0).unwrap();
    scheduler.process(1);
    assert_eq!(knob.level, 1.0, "nothing reaches the knob before its turn");
    assert_eq!(surface.get_control("level").unwrap(), 1.0.into());

    take_automation(&mut knob);
    assert_eq!(knob.level, 0.5);
    assert_eq!(surface.get_control("level").unwrap(), 0.5.into());
}

#[test]
fn a_ramp_into_a_refusing_target_counts_its_failures_without_allocating() {
    // 0 to 8 voices over 4 steps: fractions between edges are refused as
    // values an integer cannot hold, 6 and 8 by the knob itself.
    let (mut scheduler, mut knob, _) = setup(
        r#"[{ "at_step": 0, "module": "knob", "control": "voices", "value": 8.0, "ramp_steps": 4 }]"#,
    );
    let mut refused = EventCursor::new();
    for _ in 0..6 {
        let ((), allocs, frees) = allocator_events(|| step(&mut scheduler, &mut knob, 64));
        assert_eq!((allocs, frees), (0, 0));
    }
    // Each block applies its last whole value (a fraction is refused as it
    // is written); the knob refuses those past 4 and keeps what it held.
    assert_eq!(knob.voices, 3);
    assert!(refused.take(knob.cells.automation.refused()) > 2);
}

#[test]
fn a_ramp_starts_from_the_latest_write_in_the_same_block() {
    let (mut scheduler, mut knob, _) = setup(
        r#"[
            { "at_step": 0, "module": "knob", "control": "level", "value": 0.0 },
            { "at_step": 0, "module": "knob", "control": "level", "value": 1.0, "ramp_steps": 2 }
        ]"#,
    );
    step(&mut scheduler, &mut knob, 32);
    assert!(
        knob.level > 0.0 && knob.level < 0.5,
        "from 0, not 1: {}",
        knob.level
    );
    step(&mut scheduler, &mut knob, 32);
    step(&mut scheduler, &mut knob, 32);
    assert_eq!(knob.level, 1.0);
}

#[test]
fn an_event_control_cannot_be_scheduled() {
    let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
    let surface: Arc<dyn ControlSurface + Send + Sync> =
        Arc::new(DeclaredSurface::new(TABLE.clone(), cells));
    let mut map: SurfaceMap = IndexMap::new();
    map.insert("knob".to_string(), surface);
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));
    let schedule = r#"[{ "at_step": 0, "module": "knob", "control": "tap", "value": true }]"#;
    let ctrl = ControlSchedulerControls::new(parse_schedule_json(schedule).unwrap());
    let refused = ctrl.attach("sched", &directory).unwrap_err();
    assert!(refused.contains("cannot be scheduled"), "{refused}");
}

#[test]
fn a_ramp_after_an_out_of_range_jump_starts_where_the_module_clamped_it() {
    let (mut scheduler, mut knob, _) = setup(
        r#"[
            { "at_step": 0, "module": "knob", "control": "level", "value": 2.0 },
            { "at_step": 0, "module": "knob", "control": "level", "value": 0.0, "ramp_steps": 2 }
        ]"#,
    );
    knob.level = 0.0;
    step(&mut scheduler, &mut knob, 32);
    // Already below 1 a few samples in: the ramp left from 1, not from 2
    // (which the knob would hold at 1 for the first half of the ramp).
    assert!(knob.level > 0.99 && knob.level < 1.0, "{}", knob.level);
}
