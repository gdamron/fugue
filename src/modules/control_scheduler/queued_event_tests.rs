//! A scheduler writing a module that queues its events: a chord at one
//! step plays every note, in the order written, after the coalesced
//! controls, and a schedule crowding the queue with note ons still
//! releases them.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::schedule::{parse_schedule_json, SurfaceMap};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{
    take_automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, EventCursor,
    EventReserve, Refusal, RtValue,
};
use crate::invention::declared::DeclaredSurface;
use crate::ControlSurface;

const LEVEL: ControlIndex = ControlIndex(0);
const NOTE_ON: ControlIndex = ControlIndex(1);
const NOTE_OFF: ControlIndex = ControlIndex(2);

const NOTE: DeclKind = DeclKind::Number {
    min: 0.0,
    max: 127.0,
};

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Level",
    ),
    ControlDecl::new("note_on", NOTE, RtValue::F32(0.0), "Start a note").event(),
    ControlDecl::new("note_off", NOTE, RtValue::F32(0.0), "Release a note").event(),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

const CAPACITY: usize = 32;
const KEPT: usize = 4;

/// Notes what it applies, in order, with the level it then holds.
struct Keys {
    level: f32,
    played: Vec<(bool, f32, f32)>,
    cells: Arc<ControlCells>,
}

impl Module for Keys {
    fn name(&self) -> &str {
        "Keys"
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
        let RtValue::F32(number) = value else {
            return Err(Refusal::Unsupported);
        };
        match control {
            LEVEL => self.level = number,
            NOTE_ON | NOTE_OFF => {
                // Within its capacity: allocation-free.
                self.played.push((control == NOTE_ON, number, self.level));
                return Ok(RtValue::F32(0.0));
            }
            _ => return Err(Refusal::Unsupported),
        }
        Ok(value)
    }
}

fn setup(schedule_json: &str) -> (ControlScheduler, Keys) {
    let reserve = EventReserve {
        control: NOTE_OFF,
        places: KEPT,
    };
    let defaults = DECLS.iter().map(|decl| decl.default);
    let cells = Arc::new(ControlCells::queuing_events(
        defaults,
        CAPACITY,
        Some(reserve),
    ));
    let surface: Arc<dyn ControlSurface + Send + Sync> =
        Arc::new(DeclaredSurface::new(TABLE.clone(), cells.clone()));
    let mut map: SurfaceMap = IndexMap::new();
    map.insert("keys".to_string(), surface);
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));
    let ctrl = ControlSchedulerControls::new(parse_schedule_json(schedule_json).unwrap());
    ctrl.attach("sched", &directory).unwrap();
    let mut scheduler = ControlScheduler::new(48_000, ctrl);
    scheduler.settle();
    let keys = Keys {
        level: 1.0,
        played: Vec::with_capacity(4 * CAPACITY),
        cells,
    };
    (scheduler, keys)
}

/// One step's gate edge, then the keys' turn to process.
fn step(scheduler: &mut ControlScheduler, keys: &mut Keys) {
    scheduler.set_input("clock", 1.0).unwrap();
    scheduler.process(1);
    scheduler.set_input("clock", 0.0).unwrap();
    scheduler.process(15);
    take_automation(keys);
}

fn entry(step: u32, control: &str, value: f32) -> String {
    format!(
        r#"{{ "at_step": {step}, "module": "keys", "control": "{control}", "value": {value} }}"#
    )
}

fn schedule(entries: &[String]) -> String {
    format!("[{}]", entries.join(","))
}

#[test]
fn a_chord_at_one_step_plays_every_note_in_the_order_written() {
    let (mut scheduler, mut keys) = setup(&schedule(&[
        entry(0, "note_on", 60.0),
        entry(0, "note_on", 64.0),
        entry(0, "level", 0.5),
        entry(0, "note_off", 60.0),
        entry(0, "note_on", 67.0),
        entry(1, "note_on", 60.0),
    ]));
    let (_, allocs, frees) = allocator_events(|| step(&mut scheduler, &mut keys));
    assert_eq!((allocs, frees), (0, 0));
    // The level, coalesced in its slot, applies first, so every note of
    // the step plays at it.
    let expected = [
        (true, 60.0, 0.5),
        (true, 64.0, 0.5),
        (false, 60.0, 0.5),
        (true, 67.0, 0.5),
    ];
    assert_eq!(keys.played, expected);
    step(&mut scheduler, &mut keys);
    assert_eq!(keys.played[4..], [(true, 60.0, 0.5)], "the queue empties");
}

#[test]
fn note_offs_still_play_when_note_ons_crowd_the_queue() {
    let mut entries: Vec<String> = (0..CAPACITY)
        .map(|note| entry(0, "note_on", note as f32))
        .collect();
    entries.extend((0..KEPT).map(|note| entry(0, "note_off", note as f32)));
    let (mut scheduler, mut keys) = setup(&schedule(&entries));
    let mut refused = EventCursor::default();
    let (_, allocs, frees) = allocator_events(|| step(&mut scheduler, &mut keys));
    assert_eq!((allocs, frees), (0, 0));

    let ons = keys.played.iter().filter(|(on, ..)| *on).count();
    let offs: Vec<f32> = (keys.played.iter())
        .filter(|(on, ..)| !on)
        .map(|(_, note, _)| *note)
        .collect();
    assert_eq!(
        ons,
        CAPACITY - KEPT,
        "note ons stop short of the kept places"
    );
    assert_eq!(offs, [0.0, 1.0, 2.0, 3.0], "every note off fits");
    assert!(
        keys.played[..ons].iter().all(|(on, ..)| *on),
        "in the order written"
    );
    let refused_ons = refused.take(keys.cells.automation.refused());
    assert_eq!(
        refused_ons, KEPT as u32,
        "the note ons with no room, counted"
    );
}
