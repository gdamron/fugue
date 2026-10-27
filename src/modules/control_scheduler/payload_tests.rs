//! The scheduler takes its schedule as a payload: whole, between blocks,
//! without allocating or freeing, handing back the one it replaces.

use std::sync::Arc;

use indexmap::IndexMap;

use super::schedule::{parse_schedule_json, resolve_schedule, SurfaceMap};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::test_support::levels::Levels;
use crate::ControlSurface;

fn mixer_directory() -> (Levels, SurfaceMap) {
    let mixer = Levels::new();
    let mut map: SurfaceMap = IndexMap::new();
    map.insert("mixer".to_string(), mixer.surface());
    (mixer, map)
}

fn resolved(json: &str, surfaces: &SurfaceMap) -> Payload {
    let spec = parse_schedule_json(json).unwrap();
    let entries = resolve_schedule(&spec, "sched", surfaces).unwrap();
    Payload::owned(Box::new(ResolvedSchedule::new(entries)))
}

fn pulse(module: &mut ControlScheduler) {
    module.set_input("clock", 1.0).unwrap();
    module.process(1);
    module.set_input("clock", 0.0).unwrap();
    module.process(3);
}

#[test]
fn a_schedule_payload_is_adopted_without_allocating_and_the_old_one_handed_back() {
    let (mixer, surfaces) = mixer_directory();
    let ctrl = ControlSchedulerControls::new(Vec::new());
    let mut module = ControlScheduler::new(48_000, ctrl);
    let first = resolved(
        r#"[{ "at_step": 0, "module": "mixer", "control": "level.0", "value": 0.0, "ramp_steps": 4 }]"#,
        &surfaces,
    );
    let next = resolved(
        r#"[{ "at_step": 2, "module": "mixer", "control": "level.1", "value": 0.25 }]"#,
        &surfaces,
    );

    let (applied, allocs, frees) = allocator_events(|| module.apply_payload(SCHEDULE, first));
    assert_eq!((allocs, frees), (0, 0));
    assert!(applied.is_ok(), "a resolved schedule is taken");
    drop(applied);
    pulse(&mut module); // step 0: the ramp starts
    pulse(&mut module); // step 1
    let ramping = mixer.level(0);
    assert!(ramping < 1.0 && ramping > 0.0, "{ramping}");

    let (applied, allocs, frees) = allocator_events(|| module.apply_payload(SCHEDULE, next));
    assert_eq!((allocs, frees), (0, 0));
    assert!(applied.is_ok(), "the replaced schedule is handed back");
    drop(applied);
    pulse(&mut module); // step 2: the new entry fires; the old ramp is gone
    assert_eq!(mixer.level(1), 0.25);
    assert_eq!(mixer.level(0), ramping, "an abandoned ramp holds its value");
}

#[test]
fn anything_but_a_resolved_schedule_is_refused_and_handed_back() {
    let ctrl = ControlSchedulerControls::new(Vec::new());
    let mut module = ControlScheduler::new(48_000, ctrl);
    let Err((refusal, payload)) = module.apply_payload(SCHEDULE, Payload::new(1_u32)) else {
        panic!("refused");
    };
    assert_eq!(refusal, Refusal::Invalid);
    assert!(payload.is::<u32>());
    let (_, surfaces) = mixer_directory();
    let schedule = resolved("[]", &surfaces);
    let Err((refusal, _)) = module.apply_payload(ControlIndex(1), schedule) else {
        panic!("refused");
    };
    assert_eq!(refusal, Refusal::Unsupported);
}

#[test]
fn a_schedule_a_development_handed_on_is_recorded_without_resolving_again() {
    // The development delivered the payload already: what it targets may
    // have changed since, and recording it must not report the write failed.
    let (_, surfaces) = mixer_directory();
    let directory = Arc::new(std::sync::Mutex::new(surfaces));
    let ctrl = ControlSchedulerControls::new(Vec::new());
    ctrl.attach("sched", &directory).unwrap();
    let gone = r#"[{ "at_step": 0, "module": "gone", "control": "level.0", "value": 0.5 }]"#;
    assert!(ctrl.set_undeclared("schedule", gone.into()).is_ok());
    assert!(ctrl.schedule_json().contains("gone"));
    assert_eq!(ctrl.target_module_ids(), ["gone"]);
}

#[test]
fn a_retarget_of_the_schedule_it_plays_keeps_its_ramps_and_any_other_plan_replaces_it() {
    let (mixer, surfaces) = mixer_directory();
    let ramp = r#"[{ "at_step": 0, "module": "mixer", "control": "level.0", "value": 0.0, "ramp_steps": 4 }]"#;
    let resolve = |json: &str| {
        resolve_schedule(&parse_schedule_json(json).unwrap(), "sched", &surfaces).unwrap()
    };
    let retargeting =
        |json: &str| Payload::owned(Box::new(ResolvedSchedule::retargeting(resolve(json))));
    let ctrl = ControlSchedulerControls::new(Vec::new());
    let mut module = ControlScheduler::new(48_000, ctrl);
    let _ = module.apply_payload(SCHEDULE, resolved(ramp, &surfaces));
    pulse(&mut module); // step 0: the ramp starts
    pulse(&mut module); // step 1
    let reached = mixer.level(0);

    let again = retargeting(ramp);
    let (applied, allocs, frees) = allocator_events(|| module.apply_payload(SCHEDULE, again));
    assert_eq!((allocs, frees), (0, 0));
    drop(applied);
    pulse(&mut module); // step 2: the ramp carries on
    let carried = mixer.level(0);
    assert!(carried < reached && carried > 0.0, "{carried}");

    // Another plan, though marked a retarget, replaces the schedule.
    let other = r#"[{ "at_step": 9, "module": "mixer", "control": "level.1", "value": 0.5 }]"#;
    let _ = module.apply_payload(SCHEDULE, retargeting(other));
    pulse(&mut module); // step 3
    assert_eq!(mixer.level(0), carried, "the abandoned ramp holds");
}
