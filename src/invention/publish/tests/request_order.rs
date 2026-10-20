//! Ordering-only edges for conditions: a watched module ordered before the
//! target lets the request apply on its exact sample and changes nothing
//! else; an order that would change a feedback loop is not added, or kept;
//! orders go with the first change made once no condition waits.

use super::request_beats::{outcomes_of, render_counted};
use super::request_conditions::{first_change, nth_beat, rig, submit_on_beat};
use super::*;
use crate::control_request::Outcome;

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
