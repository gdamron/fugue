//! Staging on one module, without a graph: what readers see of a staged
//! write, the order a flush applies in, and what it reports.

use super::*;
use crate::control_request::outcome::{outcome_channel, OutcomeReceiver};
use crate::control_request::pending::PendingStore;
use crate::control_request::{Rank, Refusal};
use crate::payload::{RetireQueue, Retirer};
use crate::test_support::dial::{DialFactory, LEVEL, SHAPE, STEPS};
use crate::{ControlSurface, GraphModule, ModuleFactory};

fn dial() -> Box<dyn Module> {
    let built = DialFactory.build(48_000, &serde_json::json!({})).unwrap();
    match built.module {
        GraphModule::Module(module) => module,
        _ => unreachable!("a dial is a module"),
    }
}

fn store() -> (PendingStore, OutcomeReceiver) {
    let (sender, outcomes) = outcome_channel(64);
    let retirer = Retirer::new(RetireQueue::with_capacity(4), 8);
    (PendingStore::new(8, retirer, sender), outcomes)
}

fn log(outcomes: &OutcomeReceiver) -> Vec<(u64, Outcome)> {
    std::iter::from_fn(|| outcomes.try_recv())
        .map(|(id, outcome, _)| (id.0, outcome))
        .collect()
}

fn applying(id: u64, at: u64) -> Applying {
    Applying {
        id: RequestId(id),
        timed: false,
        at,
        due: at,
    }
}

fn user(due: u64) -> Standing {
    Standing::user(due)
}

/// Stages `value` on `control` for request `id`, due and applying at `at`,
/// recording it as a caller does.
fn stage(
    module: &mut dyn Module,
    outcomes: &mut Outcomes,
    write: (ControlIndex, RtValue),
    id: u64,
    (due, at): (u64, u64),
) -> bool {
    contend(module, outcomes, write, (id, user(due)), at).unwrap()
}

/// [`stage`] at any standing, reporting a loss as a caller does.
fn contend(
    module: &mut dyn Module,
    outcomes: &mut Outcomes,
    (control, value): (ControlIndex, RtValue),
    (id, standing): (u64, Standing),
    at: u64,
) -> Result<bool, Outcome> {
    outcomes.applying(applying(id, at));
    let staged = stage_write(module, control, value, standing, applying(id, at), outcomes);
    let outcome = staged.map_or_else(|lost| lost, |_| Outcome::Applied { at });
    outcomes.record(RequestId(id), false, outcome, false);
    staged
}

fn script(due: u64) -> Standing {
    Standing {
        rank: Rank::new(crate::control_request::Source::Script, 0),
        due,
    }
}

fn cells(module: &dyn Module) -> &ControlCells {
    module.declared().unwrap().1
}

#[test]
fn a_staged_write_reads_back_as_its_module_will_hold_it_and_applies_at_its_flush() {
    let mut dial = dial();
    let (mut store, received) = store();
    let outcomes = &mut store.outcomes;
    assert!(stage(
        &mut *dial,
        outcomes,
        (LEVEL, RtValue::F32(1.5)),
        1,
        (10, 10)
    ));
    assert!(stage(
        &mut *dial,
        outcomes,
        (STEPS, RtValue::I32(12)),
        2,
        (8, 10)
    ));
    let read = cells(&*dial);
    // Not applied: the cell still holds the old value, and nothing settled.
    assert_eq!(read.load(LEVEL), Some(RtValue::F32(0.25)));
    assert!(log(&received).is_empty());
    assert_eq!(outcomes.staged(), 2);
    // Read through, clamped as the dial will clamp it.
    assert_eq!(read.current(LEVEL), Some(RtValue::F32(1.0)));
    assert_eq!(read.current(STEPS), Some(RtValue::I32(8)));

    flush_staged(&mut *dial, outcomes);
    let read = cells(&*dial);
    assert_eq!(read.load(LEVEL), Some(RtValue::F32(1.0)));
    assert_eq!(read.current(LEVEL), Some(RtValue::F32(1.0)));
    assert_eq!(outcomes.staged(), 0);
    assert_eq!(
        log(&received),
        [
            (1, Outcome::Applied { at: 10 }),
            (2, Outcome::AppliedLate { at: 10, due: 8 }),
        ]
    );
}

/// A ramp starting from a control a request has staged on starts where
/// the module will be, as one starting from an automation write does.
#[test]
fn a_ramp_starts_from_a_staged_write() {
    let built = DialFactory.build(48_000, &serde_json::json!({})).unwrap();
    let GraphModule::Module(mut dial) = built.module else {
        unreachable!("a dial is a module");
    };
    let surface = built.control_surface.unwrap();
    let automation = surface.automation("level").unwrap();
    let (mut store, _received) = store();
    assert_eq!(automation.current(), Some(0.25));
    stage(
        &mut *dial,
        &mut store.outcomes,
        (LEVEL, RtValue::F32(0.75)),
        1,
        (0, 0),
    );
    assert_eq!(automation.current(), Some(0.75));
}

/// One staged at an earlier sample runs before a write at a later one is
/// weighed: each sample keeps its own setter.
#[test]
fn a_write_staged_at_an_earlier_sample_runs_before_one_at_a_later() {
    let mut dial = dial();
    let (mut store, received) = store();
    let outcomes = &mut store.outcomes;
    stage(&mut *dial, outcomes, (LEVEL, RtValue::F32(0.5)), 1, (4, 4));
    stage(
        &mut *dial,
        outcomes,
        (LEVEL, RtValue::F32(0.75)),
        2,
        (3, 10),
    );
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.5)));
    assert_eq!(log(&received), [(1, Outcome::Applied { at: 4 })]);
    flush_staged(&mut *dial, outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.75)));
    let late = Outcome::AppliedLate { at: 10, due: 3 };
    assert_eq!(log(&received), [(2, late)]);
}

/// Writes reaching one control at one sample, in either order: the one
/// due last stands, then the higher rank, then the later received. The
/// others settle at once, and only its setter runs.
#[test]
fn one_write_stands_on_a_control_at_a_sample_whatever_order_they_arrive_in() {
    let level = |value| (LEVEL, RtValue::F32(value));
    // (id, standing, value); the second stands, by its due sample, its
    // receipt, then its rank.
    let cases: [[(u64, Standing, f32); 2]; 3] = [
        [(2, user(8), 0.75), (1, user(10), 0.5)],
        [(1, user(10), 0.5), (2, user(10), 0.75)],
        [(2, script(10), 0.5), (1, user(10), 0.75)],
    ];
    for [first, second] in cases {
        for (early, late) in [(first, second), (second, first)] {
            let mut dial = dial();
            let (mut store, received) = store();
            let outcomes = &mut store.outcomes;
            let _ = contend(&mut *dial, outcomes, level(early.2), (early.0, early.1), 10);
            let _ = contend(&mut *dial, outcomes, level(late.2), (late.0, late.1), 10);
            assert_eq!(outcomes.staged(), 1);
            // The winner takes the loser's place in the order.
            assert_eq!(cells(&*dial).staged.len.load(Relaxed), 1);
            // The loser settled before anything applied.
            assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.25)));
            let (winner, loser) = (second, first);
            let lost = loser.1.rank.loses_to(winner.1.rank);
            assert_eq!(log(&received), [(loser.0, lost)], "{early:?} then {late:?}");
            flush_staged(&mut *dial, outcomes);
            assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(winner.2)));
            let applied = match winner.1.due {
                10 => Outcome::Applied { at: 10 },
                due => Outcome::AppliedLate { at: 10, due },
            };
            assert_eq!(log(&received), [(winner.0, applied)]);
        }
    }
}

/// A winner refused at its flush gives back the hold as it was before
/// either write: the one it replaced never applied.
#[test]
fn a_winner_refused_at_its_flush_gives_back_the_hold_from_before_the_write_it_replaced() {
    let mut dial = dial();
    let (mut store, received) = store();
    let outcomes = &mut store.outcomes;
    cells(&*dial).holds.take(SHAPE, script(2));
    stage(&mut *dial, outcomes, (SHAPE, RtValue::U32(1)), 1, (10, 10));
    stage(&mut *dial, outcomes, (SHAPE, RtValue::U32(7)), 2, (10, 10));
    flush_staged(&mut *dial, outcomes);
    let refused = Outcome::Refused(Refusal::Invalid);
    assert_eq!(log(&received), [(1, Outcome::Superseded), (2, refused)]);
    assert!(cells(&*dial).holds.kept(SHAPE, script(2)));
    assert_eq!(cells(&*dial).current(SHAPE), Some(RtValue::U32(0)));
}

#[test]
fn a_write_refused_at_its_flush_reports_refused_and_gives_back_its_hold() {
    let mut dial = dial();
    let (mut store, received) = store();
    let outcomes = &mut store.outcomes;
    // A script's write holds the shape; the user's replaces its hold.
    let script = script(10);
    cells(&*dial).holds.take(SHAPE, script);
    stage(&mut *dial, outcomes, (SHAPE, RtValue::U32(7)), 1, (10, 10));
    assert!(!cells(&*dial).holds.kept(SHAPE, script));
    flush_staged(&mut *dial, outcomes);
    assert_eq!(log(&received), [(1, Outcome::Refused(Refusal::Invalid))]);
    assert!(
        cells(&*dial).holds.kept(SHAPE, script),
        "the script's hold back"
    );
    assert_eq!(cells(&*dial).current(SHAPE), Some(RtValue::U32(0)));
    // Refused, it stands no more: one received before it there applies.
    stage(&mut *dial, outcomes, (SHAPE, RtValue::U32(1)), 0, (10, 10));
    flush_staged(&mut *dial, outcomes);
    assert_eq!(cells(&*dial).current(SHAPE), Some(RtValue::U32(1)));
}

#[test]
fn staging_and_flushing_neither_allocate_nor_free() {
    let mut dial = dial();
    let (mut store, received) = store();
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        let outcomes = &mut store.outcomes;
        stage(&mut *dial, outcomes, (LEVEL, RtValue::F32(0.5)), 1, (0, 0));
        stage(&mut *dial, outcomes, (STEPS, RtValue::I32(3)), 2, (0, 0));
        flush_staged(&mut *dial, outcomes);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(log(&received).len(), 2);
}

/// The worst case a block can stage: every built-in type with every plain
/// control it declares staged at once, then flushed. Nothing allocates or
/// frees, every write settles, and a full table flushes in microseconds
/// (printed with `--nocapture`).
#[test]
fn a_full_table_stages_and_flushes_without_allocating() {
    use crate::module_config::tests::registry::{base_config, UNBUILDABLE};
    let registry = crate::ModuleRegistry::default();
    let (mut widest, mut slowest) = (0, std::time::Duration::ZERO);
    for type_id in registry.types() {
        if registry.is_sink(type_id) || UNBUILDABLE.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let Ok(mut built) = registry.build(type_id, 48_000, &base_config(type_id)) else {
            continue;
        };
        let module = built.module.module_mut();
        let Some((table, cells)) = module.declared() else {
            continue;
        };
        let writes: Vec<_> = (0..table.len() as u16)
            .map(ControlIndex)
            .filter(|&index| {
                let decl = table.decl(index).map(|(decl, _)| decl);
                decl.is_some_and(|decl| !decl.event && decl.kind != DeclKind::Payload)
            })
            .filter_map(|index| Some((index, cells.load(index)?)))
            .collect();
        let (sender, received) = outcome_channel(writes.len().max(1));
        let retirer = Retirer::new(RetireQueue::with_capacity(4), 8);
        let mut store = PendingStore::new(8, retirer, sender);
        let start = std::time::Instant::now();
        let (staged, allocs, frees) = crate::alloc_counter::allocator_events(|| {
            let outcomes = &mut store.outcomes;
            let mut staged = 0;
            for (id, &write) in writes.iter().enumerate() {
                staged += usize::from(stage(&mut *module, outcomes, write, id as u64, (0, 0)));
            }
            flush_staged(&mut *module, outcomes);
            staged
        });
        slowest = slowest.max(start.elapsed());
        widest = widest.max(staged);
        assert_eq!((allocs, frees), (0, 0), "'{type_id}'");
        assert_eq!(store.outcomes.staged(), 0, "'{type_id}'");
        // Every write settles: written back, or refused by what its
        // module allows to be written at all.
        assert_eq!(log(&received).len(), writes.len(), "'{type_id}'");
    }
    assert!(widest > 8, "a table of {widest}");
    eprintln!("widest table staged: {widest} controls; slowest stage and flush: {slowest:?}");
}

#[test]
fn only_an_event_due_between_two_writes_reads_the_one_replaced() {
    let mut dial = dial();
    let (mut store, _received) = store();
    let outcomes = &mut store.outcomes;
    stage(&mut *dial, outcomes, (LEVEL, RtValue::F32(0.5)), 1, (6, 10));
    stage(
        &mut *dial,
        outcomes,
        (LEVEL, RtValue::F32(0.75)),
        2,
        (8, 10),
    );
    let between = Applying {
        due: 7,
        ..applying(3, 10)
    };
    // A write applied at once runs no replaced setter: one runs a sample.
    flush_staged_before(&mut *dial, Some(Before::write(user(7), between)), outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.25)));
    // An event due between them reads the value in force then.
    flush_staged_before(&mut *dial, Some(Before::event(between)), outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.5)));
    flush_staged(&mut *dial, outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.75)));
}

#[test]
fn a_value_replaced_at_an_earlier_sample_is_not_read_again() {
    let mut dial = dial();
    let (mut store, _received) = store();
    let outcomes = &mut store.outcomes;
    stage(&mut *dial, outcomes, (LEVEL, RtValue::F32(0.5)), 1, (6, 10));
    // A write applied at once over it, never staged in its place.
    let over = applying(2, 10);
    assert_eq!(contest(&*dial, LEVEL, user(8), over, outcomes), Ok(true));
    stage(
        &mut *dial,
        outcomes,
        (LEVEL, RtValue::F32(0.75)),
        3,
        (25, 20),
    );
    let event = Applying {
        due: 19,
        ..applying(4, 20)
    };
    flush_staged_before(&mut *dial, Some(Before::event(event)), outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.25)));
}

#[test]
fn a_write_losing_to_a_staged_one_is_read_by_an_event_after_it() {
    let mut dial = dial();
    let (mut store, _received) = store();
    let outcomes = &mut store.outcomes;
    stage(
        &mut *dial,
        outcomes,
        (LEVEL, RtValue::F32(0.75)),
        1,
        (8, 10),
    );
    let lost = Err(Outcome::Superseded);
    let write = |value| (LEVEL, RtValue::F32(value));
    assert_eq!(
        contend(&mut *dial, outcomes, write(0.5), (2, user(6)), 10),
        lost
    );
    // One due before it does not take its place.
    assert_eq!(
        contend(&mut *dial, outcomes, write(1.0), (3, user(3)), 10),
        lost
    );
    let event = Applying {
        due: 7,
        ..applying(4, 10)
    };
    flush_staged_before(&mut *dial, Some(Before::event(event)), outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.5)));
}

#[test]
fn of_two_writes_due_together_an_event_reads_the_one_that_would_set_last() {
    let between = Applying {
        due: 7,
        ..applying(9, 10)
    };
    let write = |value| (LEVEL, RtValue::F32(value));
    for (first, second, heard) in [
        (user(6), script(6), 0.5),
        (script(6), user(6), 0.1),
        (user(6), user(6), 0.1),
    ] {
        let mut dial = dial();
        let (mut store, _received) = store();
        let outcomes = &mut store.outcomes;
        contend(&mut *dial, outcomes, write(0.5), (1, first), 10).unwrap();
        stage(&mut *dial, outcomes, write(0.75), 2, (8, 10));
        let lost = contend(&mut *dial, outcomes, write(0.1), (3, second), 10);
        assert!(lost.is_err());
        flush_staged_before(&mut *dial, Some(Before::event(between)), outcomes);
        assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(heard)));
    }
}
