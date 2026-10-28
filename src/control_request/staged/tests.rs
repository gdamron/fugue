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
    (control, value): (ControlIndex, RtValue),
    id: u64,
    (due, at): (u64, u64),
) -> bool {
    outcomes.applying(applying(id, at));
    let staged = stage_write(
        module,
        control,
        value,
        user(due),
        applying(id, at),
        outcomes,
    );
    let staged = staged.unwrap();
    let outcome = Outcome::Applied { at };
    outcomes.record(RequestId(id), false, outcome, false);
    staged
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

#[test]
fn a_second_write_to_a_staged_control_flushes_the_first_so_both_setters_run() {
    let mut dial = dial();
    let (mut store, received) = store();
    let outcomes = &mut store.outcomes;
    stage(&mut *dial, outcomes, (LEVEL, RtValue::F32(0.5)), 1, (4, 10));
    stage(
        &mut *dial,
        outcomes,
        (LEVEL, RtValue::F32(0.75)),
        2,
        (10, 10),
    );
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.5)));
    assert_eq!(
        log(&received),
        [(1, Outcome::AppliedLate { at: 10, due: 4 })]
    );
    flush_staged(&mut *dial, outcomes);
    assert_eq!(cells(&*dial).load(LEVEL), Some(RtValue::F32(0.75)));
    assert_eq!(log(&received), [(2, Outcome::Applied { at: 10 })]);
}

#[test]
fn a_write_refused_at_its_flush_reports_refused_and_gives_back_its_hold() {
    let mut dial = dial();
    let (mut store, received) = store();
    let outcomes = &mut store.outcomes;
    // A script's write holds the shape; the user's replaces its hold.
    let script = Standing {
        rank: Rank::new(crate::control_request::Source::Script, 0),
        due: 10,
    };
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
