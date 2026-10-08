//! A declared module's surface: what a write does while the module is
//! building, running live, running in an offline render, or gone.

use super::declared::{dial_rig, DialFactory};
use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::Outcome;
use crate::invention::declared::{add_offline, remove_offline, retire_offline, Route};
use crate::ControlValue;

fn surface(rig: &Rig, id: &str) -> ControlSurfaceInstance {
    rig.live.control_surfaces.lock().unwrap()[id].clone()
}

fn upsert(rig: &Rig, dial: change::BuiltModule) {
    let edit = |change: &mut GraphChange| {
        change.upsert("dial", dial);
        Ok(())
    };
    rig.live.edit(edit).unwrap();
}

fn level(surface: &ControlSurfaceInstance) -> ControlValue {
    surface.get_control("level").unwrap()
}

#[test]
fn a_write_while_building_is_adopted_when_the_module_runs() {
    let mut rig = dial_rig();
    let dial = rig.build("dial", "dial", serde_json::json!({}));
    let built = dial.surface.clone().unwrap();
    built.set_control("level", 0.5.into()).unwrap();
    assert_eq!(level(&built), 0.5.into());
    assert!(
        built.set_control("pulse", true.into()).is_err(),
        "nothing to fire yet"
    );

    upsert(&rig, dial);
    assert!(rig.render(1).iter().all(|v| *v == 0.5));
}

#[test]
fn a_built_surface_takes_requests_only_once_its_change_commits() {
    let mut rig = dial_rig();
    let dial = rig.build("dial", "dial", serde_json::json!({}));
    let built = dial.surface.clone().unwrap();
    let mut change = rig.live.begin();
    change.upsert("dial", dial);
    let prepared = change.prepare().unwrap();
    let refused = built.set_control("level", 0.5.into()).unwrap_err();
    assert!(refused.contains("being installed"), "{refused}");

    rig.live.commit(prepared).unwrap();
    built.set_control("level", 0.5.into()).unwrap();
    rig.render(1);
    assert_eq!(level(&built), 0.5.into());

    // A change that never commits leaves its surfaces refusing for good.
    let dial = rig.build("dial", "dial", serde_json::json!({}));
    let orphan = dial.surface.clone().unwrap();
    let mut change = rig.live.begin();
    change.upsert("dial", dial);
    let stale = change.prepare().unwrap();
    upsert(&rig, rig.build("dial", "dial", serde_json::json!({})));
    assert!(rig.live.commit(stale).is_err());
    assert!(orphan.set_control("level", 0.5.into()).is_err());
}

#[test]
fn a_live_write_reads_back_once_the_audio_thread_applies_it() {
    let mut rig = dial_rig();
    let dial = surface(&rig, "dial");
    dial.set_control("level", 2.0.into()).unwrap();
    dial.set_control("shape", "Steep".into()).unwrap();
    assert_eq!(level(&dial), 0.25.into(), "pending, not applied");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(level(&dial), 1.0.into(), "as the module clamped it");
    assert_eq!(dial.get_control("shape").unwrap(), "steep".into());
    let applied: Vec<_> = outcomes(&mut rig).into_iter().map(|(_, o)| o).collect();
    let at = rig.graph.current_sample - 64;
    assert_eq!(applied, [Outcome::Applied { at }; 2]);
    assert!(dial.set_control("shape", "round".into()).is_err());
}

#[test]
fn live_events_are_never_coalesced() {
    let mut rig = dial_rig();
    let dial = surface(&rig, "dial");
    dial.set_control("pulse", true.into()).unwrap();
    dial.set_control("pulse", "true".into()).unwrap();
    rig.render(1);
    let out = rig.render(1);
    assert!(out.iter().all(|v| *v == 2.25), "{out:?}");
}

#[test]
fn a_replaced_or_removed_module_refuses_writes_through_its_old_surface() {
    let mut rig = dial_rig();
    let old = surface(&rig, "dial");
    let dial = rig.build("dial", "dial", serde_json::json!({}));
    upsert(&rig, dial);
    let refused = old.set_control("level", 0.75.into()).unwrap_err();
    assert!(refused.contains("removed or replaced"), "{refused}");

    let new = surface(&rig, "dial");
    new.set_control("level", 0.75.into()).unwrap();
    rig.render(1);
    assert_eq!(level(&new), 0.75.into());

    rig.live
        .edit(|change| {
            change.remove("dial");
            Ok(())
        })
        .unwrap();
    assert!(new.set_control("level", 0.5.into()).is_err());
}

#[test]
fn a_live_write_is_refused_once_the_audio_graph_is_gone() {
    let rig = dial_rig();
    let dial = surface(&rig, "dial");
    let Rig { graph, live, .. } = rig;
    drop(graph);
    let refused = dial.set_control("level", 0.5.into()).unwrap_err();
    assert!(refused.contains("stopped"), "{refused}");
    drop(live);
}

#[test]
fn an_offline_write_applies_at_once_under_the_render_lock() {
    let graph = Arc::new(Mutex::new(SignalGraph::new(
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
        MasterObservers::default(),
    )));
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let build = || {
        GraphChange::build(
            &registry,
            SAMPLE_RATE,
            "dial",
            "dial",
            &serde_json::json!({}),
        )
    };
    let surfaces = Mutex::new(IndexMap::new());
    let dial = build().unwrap();
    let first = dial.surface.clone().unwrap();
    first.set_control("level", 0.5.into()).unwrap();
    add_offline(
        &graph,
        &surfaces,
        "dial",
        dial.instance.unwrap(),
        Some(first.clone()),
    )
    .unwrap();

    first.set_control("level", 0.75.into()).unwrap();
    assert_eq!(level(&first), 0.75.into(), "applied at once");
    let module = |graph: &Arc<Mutex<SignalGraph>>| {
        let mut graph = graph.lock().unwrap();
        let module = graph.modules["dial"].module_mut();
        module.process(1);
        module.output_block(0)[0]
    };
    assert_eq!(module(&graph), 0.75);

    let second = build().unwrap();
    let surface = second.surface.clone().unwrap();
    add_offline(
        &graph,
        &surfaces,
        "dial",
        second.instance.unwrap(),
        Some(surface.clone()),
    )
    .unwrap();
    assert!(first.set_control("level", 0.5.into()).is_err(), "displaced");
    assert!(Arc::ptr_eq(&surfaces.lock().unwrap()["dial"], &surface));
    assert_eq!(
        module(&graph),
        0.25,
        "the replacement starts from its own state"
    );

    // A displaced surface stays retired even if something binds it again.
    let mut graph_lock = graph.lock().unwrap();
    first.bind(Route::Retired, graph_lock.modules["dial"].module_mut());
    drop(graph_lock);
    assert!(first.set_control("level", 0.5.into()).is_err());

    remove_offline(&graph, &surfaces, "dial").unwrap();
    assert!(surfaces.lock().unwrap().is_empty());
    assert!(surface.set_control("level", 0.5.into()).is_err());
}

#[test]
fn a_replaced_render_refuses_writes_through_its_old_surfaces() {
    let graph = Arc::new(Mutex::new(SignalGraph::new(
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
        MasterObservers::default(),
    )));
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let surfaces = Mutex::new(IndexMap::new());
    let dial = GraphChange::build(
        &registry,
        SAMPLE_RATE,
        "dial",
        "dial",
        &serde_json::json!({}),
    )
    .unwrap();
    let surface = dial.surface.clone().unwrap();
    add_offline(
        &graph,
        &surfaces,
        "dial",
        dial.instance.unwrap(),
        Some(surface.clone()),
    )
    .unwrap();
    // A controller may keep the old graph alive past its replacement, and
    // an edit through it may still be in flight.
    let late = GraphChange::build(
        &registry,
        SAMPLE_RATE,
        "late",
        "dial",
        &serde_json::json!({}),
    )
    .unwrap();
    retire_offline(&graph, &surfaces);
    assert!(surface.set_control("level", 0.5.into()).is_err());
    let late_surface = late.surface.clone().unwrap();
    assert!(add_offline(
        &graph,
        &surfaces,
        "late",
        late.instance.unwrap(),
        late.surface
    )
    .is_err());
    assert!(late_surface.set_control("level", 0.5.into()).is_err());
    assert!(!graph.lock().unwrap().modules.contains_key("late"));
    assert!(remove_offline(&graph, &surfaces, "dial").is_err());
}

#[test]
fn a_scheduled_write_reaches_a_declared_target_before_it_processes() {
    let mut rig = dial_rig();
    let schedule = serde_json::json!({
        "schedule": [{ "at": 0, "module": "dial", "control": "level", "value": 0.5 }]
    });
    let scheduler = rig.build("sched", "control_scheduler", schedule);
    let edit = |change: &mut GraphChange| {
        change.upsert("sched", scheduler);
        Ok(())
    };
    rig.live.edit(edit).unwrap();
    rig.render(1);
    rig.live.write_input("sched", "gate", 1.0).unwrap();

    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let ((), allocs, frees) =
        crate::alloc_counter::allocator_events(|| rig.graph.process_block(&mut left, &mut right));
    assert_eq!((allocs, frees), (0, 0));
    assert!(
        left.iter().all(|v| *v == 0.5),
        "from the edge's block on: {left:?}"
    );
    assert_eq!(level(&surface(&rig, "dial")), 0.5.into());
}
