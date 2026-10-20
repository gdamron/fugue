//! The scheduler takes its schedule as a payload: whole, between blocks,
//! without allocating or freeing, handing back the one it replaces.

use std::sync::Arc;

use indexmap::IndexMap;

use super::schedule::{parse_schedule_json, resolve_schedule, SurfaceMap};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::modules::mixer::MixerControls;
use crate::ControlSurface;

fn mixer_directory() -> (MixerControls, SurfaceMap) {
    let mixer = MixerControls::new(2);
    let mut map: SurfaceMap = IndexMap::new();
    map.insert(
        "mixer".to_string(),
        Arc::new(mixer.clone()) as Arc<dyn ControlSurface + Send + Sync>,
    );
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
