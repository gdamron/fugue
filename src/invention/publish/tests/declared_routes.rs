//! A declared module's surface: what a write does while the module is
//! building, running live, or gone.

use super::declared::dial_rig;
use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::Outcome;
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
