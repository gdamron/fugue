//! Ordering-only edges for conditions: a watched module ordered before the
//! target lets the request apply on its exact sample and changes nothing
//! else; an order that would change a feedback loop is not added, or kept;
//! orders go with the first change made once no condition waits.

use super::migrated::{read, write};
use super::request_beats::{outcomes_of, render_counted};
use super::request_conditions::{first_change, nth_beat, rig, submit_on_beat};
use super::*;
use crate::control_request::Outcome;
use crate::test_support::dial::DIAL;

fn watches(rig: &Rig) -> Vec<(String, String)> {
    let publisher = rig.live.publisher().lock().unwrap();
    publisher.mirror().watches.clone()
}

/// Orders `source` before `target`, returning whether it added the order.
fn order(rig: &Rig, source: &str, target: &str) -> bool {
    let mut added = false;
    rig.live
        .edit(|change| {
            added = change.order(source, target);
            Ok(())
        })
        .unwrap();
    added
}

#[test]
fn a_watched_module_ordered_first_meets_the_condition_on_its_sample() {
    // Added first, the clock computes after the dial.
    let oracle = rig(false).render(200);
    let mut rig = rig(false);
    assert!(order(&rig, "clock", "dial"));
    let id = submit_on_beat(&rig, 2);
    assert!(!order(&rig, "clock", "dial"), "once is enough");
    let out = render_counted(&mut rig, 200);
    let beat = nth_beat(&oracle, 2);
    assert_ne!(beat % 64, 0, "the beat falls inside a block");
    // The order changes nothing else: the render matches up to the beat.
    assert_eq!(first_change(&out, &oracle), beat);
    assert_eq!(
        outcomes_of(&mut rig, id),
        [(id, Outcome::Applied { at: beat })]
    );
}

#[test]
fn an_order_closing_a_cycle_is_not_added() {
    let rig = rig(true);
    let osc = rig.build("osc", "oscillator", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("osc", osc);
            change.connect(edge("dial", "out", "osc", "frequency_mod"))
        })
        .unwrap();
    let generation = rig.live.generation();
    assert!(!order(&rig, "osc", "dial"));
    assert!(!order(&rig, "dial", "osc"), "routed already");
    assert!(!order(&rig, "dial", "dial"));
    assert_eq!(rig.live.generation(), generation);
    assert!(watches(&rig).is_empty());
}

/// Adds an oscillator `id` and routes each `(from, to)` pair, output `audio`
/// into input `frequency_mod`.
fn add_oscs(rig: &Rig, ids: &[&str], routes: &[(&str, &str)]) {
    let built: Vec<_> = ids
        .iter()
        .map(|id| (*id, rig.build(id, "oscillator", serde_json::json!({}))))
        .collect();
    rig.live
        .edit(|change| {
            for (id, osc) in built {
                change.upsert(id, osc);
            }
            for (from, to) in routes {
                change.connect(edge(from, "audio", to, "frequency_mod"))?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_route_closing_a_cycle_through_an_order_drops_it() {
    let rig = rig(false);
    add_oscs(&rig, &["osc"], &[]);
    assert!(order(&rig, "osc", "dial"));
    submit_on_beat(&rig, 1_000);
    rig.live
        .edit(|change| change.connect(edge("dial", "out", "osc", "frequency_mod")))
        .unwrap();
    assert!(watches(&rig).is_empty(), "the order would fold a loop");
}

#[test]
fn an_order_moving_a_loops_delayed_route_is_not_added() {
    // The search enters the loop at osc_a, so osc_b -> osc_a is delayed;
    // dial, added earlier, ordered before osc_b would enter it there.
    let rig = rig(false);
    let routes = [("osc_a", "osc_b"), ("osc_b", "osc_a")];
    add_oscs(&rig, &["osc_a", "osc_b"], &routes);
    let generation = rig.live.generation();
    assert!(!order(&rig, "dial", "osc_b"));
    assert_eq!(rig.live.generation(), generation);
    assert!(order(&rig, "dial", "osc_a"), "entered where it already is");
}

#[test]
fn an_order_reordering_two_writers_of_a_control_is_not_added() {
    // Of two schedulers writing the dial's level, the later computed wins:
    // sched_a, added first, computes after sched_b.
    let rig = rig(false);
    let schedule = |value| serde_json::json!({ "schedule": [step("dial", "level", value)] });
    let a = rig.build("sched_a", "control_scheduler", schedule(0.25));
    let b = rig.build("sched_b", "control_scheduler", schedule(0.75));
    rig.live
        .edit(|change| {
            change.upsert("sched_a", a);
            change.upsert("sched_b", b);
            Ok(())
        })
        .unwrap();
    assert!(!order(&rig, "sched_a", "sched_b"), "sched_b would win");
    assert!(order(&rig, "sched_b", "sched_a"), "the order they run in");
}

/// A schedule entry writing `value` to `control` of `module` on step 0.
fn step(module: &str, control: &str, value: f32) -> serde_json::Value {
    serde_json::json!({ "at_step": 0, "module": module, "control": control, "value": value })
}

/// sched_a sets the clock's tempo, so the two run as a loop; it and
/// sched_b, clocked by clock_b, both write the dial's level.
fn writers_loop() -> Rig {
    let rig = rig(false);
    let schedule = |at: serde_json::Value| serde_json::json!({ "schedule": at });
    let sched_a = schedule(serde_json::json!([
        step("clock", "bpm", 90.0),
        step("dial", "level", 0.25)
    ]));
    let sched_a = rig.build("sched_a", "control_scheduler", sched_a);
    let clock_b = rig.build("clock_b", "clock", serde_json::json!({}));
    let sched_b = schedule(serde_json::json!([step("dial", "level", 0.75)]));
    let sched_b = rig.build("sched_b", "control_scheduler", sched_b);
    rig.live
        .edit(|change| {
            change.upsert("sched_a", sched_a);
            change.upsert("clock_b", clock_b);
            change.upsert("sched_b", sched_b);
            change.connect(edge("clock", "beat", "sched_a", "clock"))?;
            change.connect(edge("clock_b", "beat", "sched_b", "clock"))
        })
        .unwrap();
    rig
}

#[test]
fn an_order_running_a_writers_loop_earlier_is_not_added() {
    // Ordering the clock before clock_b moves the loop past sched_b, and
    // so which of the two schedulers writes the dial's level last.
    let oracle = writers_loop().render(2_000);
    let mut rig = writers_loop();
    assert!(!order(&rig, "clock", "clock_b"), "the other would win");
    submit_on_beat(&rig, 100_000);
    assert!(rig.render(2_000) == oracle, "unchanged while it waits");
}

/// sched_b, retargeted live to write clock2's tempo and d2's level, which
/// publishes the topology recompiled to run it, with clock2 and sched_a,
/// ahead (FUG-388).
fn retargeted_live() -> Rig {
    let rig = rig(false);
    let schedule = |at: serde_json::Value| serde_json::json!({ "schedule": at });
    let sched_b = schedule(serde_json::json!([step("clock", "bpm", 90.0)]));
    let sched_b = rig.build("sched_b", "control_scheduler", sched_b);
    let clock2 = rig.build("clock2", "clock", serde_json::json!({ "bpm": 1100.0 }));
    let d2 = rig.build("d2", DIAL, serde_json::json!({}));
    let sched_a = schedule(serde_json::json!([step("d2", "level", 0.25)]));
    let sched_a = rig.build("sched_a", "control_scheduler", sched_a);
    rig.live
        .edit(|change| {
            change.upsert("sched_b", sched_b);
            change.upsert("clock2", clock2);
            change.upsert("d2", d2);
            change.upsert("sched_a", sched_a);
            change.connect(edge("clock", "beat", "sched_b", "clock"))?;
            change.connect(edge("clock2", "beat", "sched_a", "clock"))
        })
        .unwrap();
    let retarget = serde_json::json!([step("clock2", "bpm", 1100.0), step("d2", "level", 0.75)]);
    write(&rig, "sched_b", "schedule", retarget.to_string().into());
    rig
}

#[test]
fn an_order_after_a_live_retarget_is_added() {
    // The installed topology is the one a compile now makes, so an order
    // is judged on its own, and the graph plays as with none asked for.
    // (Which writer lands is not this test's: played by the drain, the two
    // schedulers' writes at one sample land in module order.)
    let mut oracle = retargeted_live();
    oracle.render(200);
    let mut rig = retargeted_live();
    let generation = rig.live.generation();
    assert!(order(&rig, "clock", "dial"));
    assert_eq!(rig.live.generation(), generation + 1, "the order alone");
    assert!(watches(&rig).contains(&("clock".to_string(), "dial".to_string())));
    rig.render(200);
    assert_eq!(read(&rig, "d2", "level"), read(&oracle, "d2", "level"));
}

/// sched_b, clocked by the clock, writes the dial's level; sched_a,
/// never clocked, writes d2's, which reaches the dac. Then sched_b is
/// retargeted live to write d2's level on its first two steps.
pub(super) fn retargeted_onto_a_later_module() -> Rig {
    let rig = rig(false);
    let schedule = |at: serde_json::Value| serde_json::json!({ "schedule": at });
    let sched_b = schedule(serde_json::json!([step("dial", "level", 0.5)]));
    let sched_b = rig.build("sched_b", "control_scheduler", sched_b);
    let d2 = rig.build("d2", DIAL, serde_json::json!({}));
    let sched_a = schedule(serde_json::json!([step("d2", "level", 0.25)]));
    let sched_a = rig.build("sched_a", "control_scheduler", sched_a);
    rig.live
        .edit(|change| {
            change.upsert("sched_b", sched_b);
            change.upsert("d2", d2);
            change.upsert("sched_a", sched_a);
            change.connect(edge("clock", "beat", "sched_b", "clock"))?;
            change.connect(edge("d2", "out", "dac", "audio"))
        })
        .unwrap();
    let mut later = step("d2", "level", 0.5);
    later["at_step"] = 1.into();
    let retarget = serde_json::json!([step("d2", "level", 0.75), later]);
    write(&rig, "sched_b", "schedule", retarget.to_string().into());
    rig
}

#[test]
fn an_order_after_a_retarget_onto_a_later_module_is_added() {
    // The retarget published the recompile running sched_b before d2, so
    // the order moves nothing.
    let oracle = retargeted_onto_a_later_module().render(200);
    let mut rig = retargeted_onto_a_later_module();
    assert!(order(&rig, "clock", "dial"));
    assert!(rig.render(200) == oracle, "unchanged");
}

#[test]
fn orders_go_with_the_first_change_once_no_condition_waits() {
    let mut rig = rig(false);
    let unplug = |rig: &Rig, module: &str| {
        rig.live
            .edit(|change| {
                change.disconnect(edge(module, "beat", "dac", "audio"));
                change.disconnect(edge(module, "out", "dac", "audio"));
                Ok(())
            })
            .unwrap();
    };
    order(&rig, "clock", "dial");
    submit_on_beat(&rig, 1_000);
    unplug(&rig, "clock");
    assert_eq!(watches(&rig).len(), 1, "kept while a condition waits");

    // Refused once its clock goes, the condition frees its slot.
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();
    render_counted(&mut rig, 1);
    assert_eq!(rig.live.requests.watch_slots().taken(), 0);
    unplug(&rig, "dial");
    assert!(watches(&rig).is_empty());
}

/// A block size change recompiles in place on the audio thread: it keeps
/// the orders the topology was published with, so a condition waiting on
/// one still applies on its exact sample (FUG-391).
#[test]
fn a_block_size_change_keeps_the_orders_installed() {
    let oracle = rig(false).render(200);
    let mut rig = rig(false);
    assert!(order(&rig, "clock", "dial"));
    let id = submit_on_beat(&rig, 2);
    let mut out = render_counted(&mut rig, 1);
    rig.graph.set_block_size(128);
    out.extend(rig.render(199));
    let beat = nth_beat(&oracle, 2);
    assert_ne!(beat % 64, 0, "the beat falls inside a block");
    assert_eq!(first_change(&out, &oracle), beat);
    assert_eq!(
        outcomes_of(&mut rig, id),
        [(id, Outcome::Applied { at: beat })]
    );
}
