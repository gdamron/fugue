//! A schedule's entries and ramps into every legacy family play on the
//! audio thread without allocating, freeing or waiting on a lock a control
//! thread holds: a write finding one held waits, and lands a block later.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{scheduled_surface, while_held};
use crate::invention::runtime::ControlSurfaceInstance;
use crate::module_config::tests::registry::eight_frame_wav;
use crate::modules::sample_instrument::ZoneSpec;
use crate::modules::{
    CellSequencerControls, MelodyControls, SampleInstrumentControls, SamplePlayerControls,
};
use crate::ControlValue;

/// The locking families' controls, kept to hold their locks.
pub(super) struct Locking {
    pub(super) melody: MelodyControls,
    player: SamplePlayerControls,
    pub(super) instrument: SampleInstrumentControls,
    cells: CellSequencerControls,
}

impl Locking {
    pub(super) fn new() -> Self {
        let wav = eight_frame_wav();
        let zone = ZoneSpec {
            root_note: 60,
            key_low: 0,
            key_high: 127,
            asset: wav.to_string(),
            level: 1.0,
            loop_spec: None,
        };
        let cells = CellSequencerControls::new();
        cells.set_cells_json("[[60], [62], [64]]").unwrap();
        Self {
            melody: MelodyControls::new(60, vec![0, 2, 4]),
            player: SamplePlayerControls::with_mode(48_000, Some(wav), None, None, false).unwrap(),
            instrument: SampleInstrumentControls::new(48_000, 0.1, vec![zone])
                .unwrap()
                .0,
            cells,
        }
    }

    /// A directory of every legacy family: the atomic ones fresh, the
    /// locking ones these.
    pub(super) fn directory(&self) -> SurfaceDirectory {
        let mut map: IndexMap<String, ControlSurfaceInstance> = IndexMap::new();
        for type_id in [
            "adsr",
            "filter",
            "lfo",
            "mixer",
            "sample_slicer",
            "sample_kit",
        ] {
            map.insert(type_id.into(), scheduled_surface(type_id));
        }
        map.insert("melody".into(), Arc::new(self.melody.clone()));
        map.insert("sample_player".into(), Arc::new(self.player.clone()));
        map.insert(
            "sample_instrument".into(),
            Arc::new(self.instrument.clone()),
        );
        map.insert("cell_sequencer".into(), Arc::new(self.cells.clone()));
        Arc::new(Mutex::new(map))
    }
}

pub(super) fn entry(
    at_step: u64,
    module: &str,
    control: &str,
    value: ScheduleValue,
) -> ScheduleEntry {
    ScheduleEntry {
        at_step,
        module: module.into(),
        control: control.into(),
        value,
        ramp_steps: None,
    }
}

fn ramp(module: &str, control: &str, to: f32) -> ScheduleEntry {
    ScheduleEntry {
        ramp_steps: Some(2),
        ..entry(0, module, control, ScheduleValue::Number(to))
    }
}

/// A scheduler over `directory` running `entries`.
pub(super) fn scheduler(
    entries: Vec<ScheduleEntry>,
    directory: &SurfaceDirectory,
) -> ControlScheduler {
    let controls = ControlSchedulerControls::new(entries);
    controls.attach("sched", directory).unwrap();
    let mut scheduler = ControlScheduler::new(48_000, controls);
    scheduler.settle();
    scheduler
}

/// One block with the clock at `gate`, counted.
pub(super) fn block(scheduler: &mut ControlScheduler, gate: f32) -> (usize, usize) {
    scheduler.set_input("clock", gate).unwrap();
    let (_, allocs, frees) = allocator_events(|| scheduler.process(64));
    (allocs, frees)
}

fn number(directory: &SurfaceDirectory, module: &str, control: &str) -> f32 {
    let surface = directory.lock().unwrap()[module].clone();
    match surface.get_control(control).unwrap() {
        ControlValue::Number(number) => number,
        other => panic!("{module}.{control}: {other:?}"),
    }
}

#[test]
fn entries_and_ramps_into_every_legacy_family_allocate_nothing() {
    use ScheduleValue::{Bool, Number};
    let locking = Locking::new();
    let directory = locking.directory();
    let entries = vec![
        ramp("adsr", "attack", 0.5),
        entry(0, "filter", "cutoff", Number(500.0)),
        ramp("lfo", "rate", 4.0),
        ramp("mixer", "level.0", 0.25),
        entry(0, "sample_slicer", "time_ratio", Number(2.0)),
        ramp("sample_kit", "level.0", 0.5),
        ramp("melody", "degree.0", 7.0),
        entry(0, "melody", "degree_count", Number(2.0)),
        entry(0, "sample_player", "play", Bool(true)),
        ramp("sample_player", "pitch_ratio", 2.0),
        entry(0, "sample_instrument", "note_on", Number(64.0)),
        ramp("sample_instrument", "level.0", 0.5),
        entry(0, "cell_sequencer", "select_cell", Number(1.0)),
        entry(0, "cell_sequencer", "follow_cell_length", Bool(true)),
        ramp("cell_sequencer", "gate_length", 0.25),
    ];
    let mut scheduler = scheduler(entries, &directory);
    for gate in [1.0, 0.0, 1.0, 0.0, 1.0, 0.0] {
        assert_eq!(block(&mut scheduler, gate), (0, 0));
    }
    for (module, control, to) in [
        ("adsr", "attack", 0.5),
        ("filter", "cutoff", 500.0),
        ("lfo", "rate", 4.0),
        ("mixer", "level.0", 0.25),
        ("sample_slicer", "time_ratio", 2.0),
        ("sample_kit", "level.0", 0.5),
        ("melody", "degree.0", 7.0),
        ("melody", "degree_count", 2.0),
        ("sample_player", "pitch_ratio", 2.0),
        ("sample_instrument", "level.0", 0.5),
        ("cell_sequencer", "select_cell", 1.0),
        ("cell_sequencer", "gate_length", 0.25),
    ] {
        let landed = number(&directory, module, control);
        assert!((landed - to).abs() < 1e-4, "{module}.{control}: {landed}");
    }
    assert!(locking.player.play());
    assert!(locking.cells.follow_cell_length());
    assert_eq!(locking.instrument.notes_version(), 1);
}

/// The instrument's queued note events, as (note, on).
pub(super) fn queued_notes(instrument: &SampleInstrumentControls) -> Vec<(u8, bool)> {
    let pending = instrument.pending_notes().lock().unwrap();
    pending.iter().map(|event| (event.note, event.on)).collect()
}

/// `run` with every locking family's lock held, as control threads would.
fn with_locks_held<T>(locking: &Locking, run: impl FnOnce() -> T) -> T {
    while_held(&locking.melody.table, || {
        while_held(&locking.player.shared, || {
            while_held(locking.instrument.pending_notes(), || {
                while_held(&locking.cells.shared.sequences, run)
            })
        })
    })
}

#[test]
fn writes_finding_their_locks_held_land_a_block_late_in_order() {
    use ScheduleValue::{Bool, Number};
    let locking = Locking::new();
    let directory = locking.directory();
    let mut scheduler = scheduler(
        vec![
            entry(3, "melody", "degree.1", Number(5.0)),
            entry(3, "sample_player", "play", Bool(true)),
            entry(3, "sample_instrument", "note_on", Number(64.0)),
            entry(3, "cell_sequencer", "select_cell", Number(2.0)),
            // A ramp starting while the table is held still starts.
            ScheduleEntry {
                ramp_steps: Some(1),
                ..entry(3, "melody", "note_weight.0", Number(3.0))
            },
            entry(4, "melody", "degree.1", Number(6.0)),
            entry(4, "sample_player", "loop", Bool(true)),
            entry(4, "sample_instrument", "note_off", Number(64.0)),
            entry(5, "melody", "degree.2", Number(9.0)),
        ],
        &directory,
    );
    for gate in [1.0, 0.0, 1.0, 0.0, 1.0, 0.0] {
        assert_eq!(block(&mut scheduler, gate), (0, 0), "steps 0 to 2");
    }
    // Steps 3 and 4 play while control threads hold every family's lock.
    let held = with_locks_held(&locking, || {
        [1.0, 0.0, 1.0].map(|gate| block(&mut scheduler, gate))
    });
    assert_eq!(held, [(0, 0); 3], "steps 3 and 4");
    assert_eq!(locking.melody.degree(1), Ok(2), "waiting, not waited for");
    assert!(!locking.player.play());
    assert!(!locking.player.loop_enabled());
    assert_eq!(queued_notes(&locking.instrument), []);
    assert_eq!(locking.cells.selected_cell(), 2, "an atomic write lands");
    // Released: the first block after lands every write, in order.
    assert_eq!(block(&mut scheduler, 0.0), (0, 0), "released");
    assert_eq!(locking.melody.degree(1), Ok(6), "the newer value");
    assert_eq!(locking.melody.note_weight(0), Ok(3.0), "the ramp landed");
    assert!(locking.player.play());
    assert!(locking.player.loop_enabled());
    assert_eq!(
        queued_notes(&locking.instrument),
        [(64, true), (64, false)],
        "the note on, then its note off: no stuck note"
    );
    let writes = scheduler.legacy.as_ref().unwrap();
    assert_eq!((writes.waiting(), writes.refused().count()), (0, 0));
    // The next step's entries land as ever.
    assert_eq!(block(&mut scheduler, 1.0), (0, 0), "step 5");
    assert_eq!(locking.melody.degree(2), Ok(9));
}

#[test]
fn a_note_off_waiting_through_a_retarget_still_lands() {
    use super::resolved::ResolvedSchedule;
    use super::schedule::resolve_schedule;
    use crate::payload::Payload;
    let locking = Locking::new();
    let directory = locking.directory();
    let entries = vec![
        entry(
            1,
            "sample_instrument",
            "note_on",
            ScheduleValue::Number(64.0),
        ),
        entry(
            2,
            "sample_instrument",
            "note_off",
            ScheduleValue::Number(64.0),
        ),
    ];
    let mut scheduler = scheduler(entries.clone(), &directory);
    for gate in [1.0, 0.0, 1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    let surfaces = directory.lock().unwrap().clone();
    let retarget = resolve_schedule(&entries, "sched", &surfaces).unwrap();
    let retarget = Payload::owned(Box::new(ResolvedSchedule::retargeting(retarget)));
    // Step 2's note_off finds the queue held, and is still waiting when the
    // same plan, re-resolved, is adopted.
    let old = while_held(locking.instrument.pending_notes(), || {
        block(&mut scheduler, 1.0);
        scheduler.apply_payload(SCHEDULE, retarget)
    });
    drop(old);
    assert_eq!(block(&mut scheduler, 0.0), (0, 0), "released");
    assert_eq!(
        queued_notes(&locking.instrument),
        [(64, true), (64, false)],
        "no stuck note"
    );
}

#[test]
fn a_ramp_after_a_write_finding_its_table_held_starts_from_that_write() {
    let locking = Locking::new();
    let directory = locking.directory();
    let mut scheduler = scheduler(
        vec![
            entry(1, "melody", "degree.0", ScheduleValue::Number(12.0)),
            ScheduleEntry {
                ramp_steps: Some(4),
                ..entry(1, "melody", "degree.0", ScheduleValue::Number(0.0))
            },
        ],
        &directory,
    );
    block(&mut scheduler, 1.0);
    block(&mut scheduler, 0.0);
    let held = while_held(&locking.melody.table, || block(&mut scheduler, 1.0));
    assert_eq!(held, (0, 0), "step 1");
    assert_eq!(block(&mut scheduler, 0.0), (0, 0), "released");
    // The ramp starts at 12, not at the 0 the table held: it falls from 12.
    let started = locking.melody.degree(0).unwrap();
    assert!((8..=12).contains(&started), "{started}");
    for gate in [1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    let falling = locking.melody.degree(0).unwrap();
    assert!((1..started).contains(&falling), "{falling}");
}

#[test]
fn a_write_another_waiting_one_needs_is_not_merged_away() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let mut scheduler = scheduler(
        vec![
            entry(1, "melody", "degree_count", Number(1.0)),
            // Degree 2 is there only once the count grows back to 3.
            entry(2, "melody", "degree_count", Number(3.0)),
            entry(2, "melody", "degree.2", Number(9.0)),
            entry(2, "melody", "degree_count", Number(4.0)),
        ],
        &directory,
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    while_held(&locking.melody.table, || block(&mut scheduler, 1.0));
    block(&mut scheduler, 0.0);
    assert_eq!(number(&directory, "melody", "degree_count"), 4.0);
    assert_eq!(locking.melody.degree(2), Ok(9));
    let writes = scheduler.legacy.as_ref().unwrap();
    assert_eq!(writes.refused().count(), 0);
}

#[test]
fn a_note_off_waiting_when_its_schedule_is_replaced_still_lands() {
    use super::resolved::ResolvedSchedule;
    use super::schedule::resolve_schedule;
    use crate::payload::Payload;
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let mut scheduler = scheduler(
        vec![
            entry(1, "sample_instrument", "note_on", Number(64.0)),
            entry(2, "sample_instrument", "note_off", Number(64.0)),
        ],
        &directory,
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    // The new schedule writes the melody only: the instrument is not its.
    let surfaces = directory.lock().unwrap().clone();
    let melody = vec![entry(0, "melody", "degree.0", Number(3.0))];
    let next = ResolvedSchedule::new(resolve_schedule(&melody, "sched", &surfaces).unwrap());
    let next = Payload::owned(Box::new(next));
    // Step 2's note_off finds the queue held, and is still waiting when the
    // new schedule is adopted.
    let (old, allocs, frees) = while_held(locking.instrument.pending_notes(), || {
        block(&mut scheduler, 1.0);
        allocator_events(|| scheduler.apply_payload(SCHEDULE, next))
    });
    assert_eq!((allocs, frees), (0, 0), "adopted");
    drop(old);
    assert_eq!(block(&mut scheduler, 0.0), (0, 0), "released");
    assert_eq!(
        queued_notes(&locking.instrument),
        [(64, true), (64, false)],
        "no stuck note"
    );
    assert_eq!(scheduler.draining[0].refused().count(), 0);
}

#[test]
fn two_schedulers_notes_to_one_instrument_land_in_the_order_they_were_made() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let note = |control| vec![entry(1, "sample_instrument", control, Number(64.0))];
    let mut first = scheduler(note("note_on"), &directory);
    let mut second = scheduler(note("note_off"), &directory);
    for gate in [1.0, 0.0] {
        block(&mut first, gate);
        block(&mut second, gate);
    }
    // The first's note_on finds the queue held; the second's note_off,
    // made after it, finds it free.
    while_held(locking.instrument.pending_notes(), || {
        block(&mut first, 1.0)
    });
    assert_eq!(block(&mut second, 1.0), (0, 0));
    assert_eq!(queued_notes(&locking.instrument), [], "behind the note_on");
    for gate in [0.0, 0.0] {
        assert_eq!(block(&mut first, gate), (0, 0));
        assert_eq!(block(&mut second, gate), (0, 0));
    }
    assert_eq!(
        queued_notes(&locking.instrument),
        [(64, true), (64, false)],
        "no stuck note"
    );
}

#[test]
fn two_ramps_waiting_behind_a_note_leave_room_for_its_note_off() {
    use ScheduleValue::Number;
    let locking = Locking::new();
    let directory = locking.directory();
    let wav = eight_frame_wav();
    let zone = |root_note| ZoneSpec {
        root_note,
        key_low: root_note,
        key_high: root_note,
        asset: wav.to_string(),
        level: 1.0,
        loop_spec: None,
    };
    let (instrument, _) =
        SampleInstrumentControls::new(48_000, 0.1, vec![zone(60), zone(64)]).unwrap();
    let surface: ControlSurfaceInstance = Arc::new(instrument.clone());
    directory.lock().unwrap().insert("zones".into(), surface);
    let ramp = |control, to| ScheduleEntry {
        ramp_steps: Some(4),
        ..entry(1, "zones", control, Number(to))
    };
    let mut scheduler = scheduler(
        vec![
            ramp("level.0", 0.5),
            ramp("level.1", 0.25),
            entry(2, "zones", "note_on", Number(64.0)),
            entry(3, "zones", "note_off", Number(64.0)),
        ],
        &directory,
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        block(&mut scheduler, gate);
    }
    // Steps 2 and 3 play with the queue held: both ramps write behind the
    // note_on every sample.
    let held = while_held(instrument.pending_notes(), || {
        [1.0, 0.0, 1.0].map(|gate| block(&mut scheduler, gate))
    });
    assert_eq!(held, [(0, 0); 3]);
    block(&mut scheduler, 0.0);
    assert_eq!(queued_notes(&instrument), [(64, true), (64, false)]);
}

#[test]
fn writes_that_never_land_are_counted_in_a_control() {
    use crate::ControlSurface;
    let locking = Locking::new();
    let directory = locking.directory();
    let mut scheduler = scheduler(
        vec![entry(1, "melody", "degree.1", ScheduleValue::Number(5.0))],
        &directory,
    );
    block(&mut scheduler, 1.0);
    block(&mut scheduler, 0.0);
    // Held through more than 10 ms of blocks: the value gives up.
    let mut gates = [0.0; 10];
    gates[0] = 1.0;
    let held = while_held(&locking.melody.table, || {
        gates.map(|gate| block(&mut scheduler, gate))
    });
    assert_eq!(held, [(0, 0); 10]);
    assert_eq!(locking.melody.degree(1), Ok(2), "never landed");
    let ctrl = scheduler.controls().clone();
    assert_eq!(ctrl.refused_writes(), 1);
    assert_eq!(
        ctrl.get_control("refused_writes"),
        Ok(ControlValue::Number(1.0))
    );
    assert_eq!(Module::get_control(&scheduler, "refused_writes"), Ok(1.0));
    assert!(ctrl.set_control("refused_writes", 0.0.into()).is_err());
    assert_eq!(ctrl.crowded_ramps(), 0);
}
