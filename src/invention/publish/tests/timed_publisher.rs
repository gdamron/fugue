//! The publisher's side of a timed structural edit: it stays at the
//! generation the edit was prepared against until the audio thread lands
//! it, adopts it then, and never lets another change apply it to a graph
//! it was not prepared for.

use super::request_beats::{outcomes_of, render_counted};
use super::*;
use crate::control_request::{Outcome, Refusal, RequestId, When};
use crate::invention::publish::publisher::{EDIT_RESERVE, REQUEST_QUEUE_CAPACITY};

/// Queues `prepared` to land `when`, refused once `ttl` samples pass.
fn time(rig: &Rig, prepared: PreparedChange, when: When, ttl: Option<u64>) -> RequestId {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let retain = (Box::new(|_: &mut RuntimeState| {}) as _, rig.live.port());
    let Ok(id) = publisher.publish_timed(prepared, when, ttl, retain) else {
        panic!("the timed edit was refused");
    };
    id
}

/// The flag the reclaimer reads before taking the publisher.
fn waiting(publisher: &Publisher) -> bool {
    publisher
        .timed_flag()
        .load(std::sync::atomic::Ordering::Relaxed)
}

/// A change removing `id`, prepared against the graph that is playing.
fn removing(rig: &Rig, id: &str) -> PreparedChange {
    let publisher = rig.live.publisher().lock().unwrap();
    let mut change = rig.live.change_on(&publisher);
    change.remove(id);
    change.prepare().unwrap()
}

/// A change toggling osc1's modulation of osc2, every other one connecting.
fn toggle(rig: &Rig, n: usize) -> PreparedChange {
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    let publisher = rig.live.publisher().lock().unwrap();
    let mut change = rig.live.change_on(&publisher);
    if n.is_multiple_of(2) {
        change.connect(fm).unwrap();
    } else {
        change.disconnect(fm);
    }
    change.prepare().unwrap()
}

#[test]
fn a_landed_edit_is_adopted_and_until_then_the_publisher_stays_put() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let generation = rig.live.publisher().lock().unwrap().generation();
    let at = rig.graph.current_sample + 64 * 2 + 5;
    let id = time(&rig, removing(&rig, "osc2"), When::AtSample(at), None);
    {
        let mut publisher = rig.live.publisher().lock().unwrap();
        assert!(publisher.adopt().is_none(), "waiting");
        assert!(publisher.timed_waiting());
        assert!(waiting(&publisher), "the reclaimer's flag follows");
        assert_eq!(publisher.generation(), generation);
    }

    render_counted(&mut rig, 4);
    assert_eq!(outcomes_of(&mut rig, id), [(id, Outcome::Applied { at })]);
    assert_eq!(rig.module_ids(), ["osc1", "dac"]);
    let mut publisher = rig.live.publisher().lock().unwrap();
    let adopted = publisher.adopt().expect("landed");
    assert!(adopted.published.previous.modules.contains_key("osc2"));
    assert!(!publisher.mirror().modules.contains_key("osc2"));
    assert_eq!(publisher.generation(), generation + 1);
    assert!(!publisher.timed_waiting());
    assert!(!waiting(&publisher));
    assert!(publisher.adopt().is_none(), "adopted once");
}

#[test]
fn a_change_published_while_an_edit_waits_cancels_it() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let at = rig.graph.current_sample + 64 * 4;
    let id = time(&rig, removing(&rig, "osc2"), When::AtSample(at), None);
    // Prepared against the same graph, published first: it stands.
    let prepared = removing(&rig, "osc1");
    let mut publisher = rig.live.publisher().lock().unwrap();
    assert!(publisher.publish(prepared).is_ok());
    assert!(!publisher.timed_waiting(), "cancelled");
    assert!(!waiting(&publisher));
    drop(publisher);

    render_counted(&mut rig, 8);
    let refused = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(outcomes_of(&mut rig, id), [(id, refused)]);
    assert_eq!(rig.module_ids(), ["osc2", "dac"]);
}

#[test]
fn a_change_prepared_before_a_landing_is_refused_after_it() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    time(&rig, removing(&rig, "osc2"), When::AfterSamples(10), None);
    let stale = removing(&rig, "osc1");
    render_counted(&mut rig, 1);

    // Landed and not yet adopted: refused as moved, nothing changed.
    let mut publisher = rig.live.publisher().lock().unwrap();
    let Err(refused) = publisher.publish(stale) else {
        panic!("a stale change published");
    };
    assert!(matches!(refused.error, GraphCommandError::TopologyMoved));
    assert!(publisher.adopt().is_some(), "still the edit's to adopt");
    drop((refused, publisher));
    assert_eq!(rig.module_ids(), ["osc1", "dac"]);
}

#[test]
fn a_change_refused_for_a_full_queue_leaves_a_timed_edit_waiting() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    // Every slot but one taken, then the timed edit takes that one.
    for n in 0..REQUEST_QUEUE_CAPACITY + EDIT_RESERVE - 1 {
        let prepared = toggle(&rig, n);
        let mut publisher = rig.live.publisher().lock().unwrap();
        assert!(publisher.publish(prepared).is_ok());
    }
    let at = rig.graph.current_sample + 64 * 400;
    let id = time(&rig, removing(&rig, "osc2"), When::AtSample(at), None);

    let prepared = toggle(&rig, 1);
    let mut publisher = rig.live.publisher().lock().unwrap();
    let Err(refused) = publisher.publish(prepared) else {
        panic!("published into a full queue");
    };
    assert!(matches!(refused.error, GraphCommandError::QueueFull));
    assert!(publisher.timed_waiting(), "a refused change cancelled it");
    drop((refused, publisher));

    let mut landed = Vec::new();
    for _ in 0..410 {
        rig.live.reclaim();
        render_counted(&mut rig, 1);
        landed.extend(outcomes_of(&mut rig, id));
    }
    assert_eq!(landed, [(id, Outcome::Applied { at })]);
    assert_eq!(rig.module_ids(), ["osc1", "dac"]);
}

#[test]
fn an_edit_that_expires_is_forgotten() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let at = rig.graph.current_sample + 64 * 4;
    let id = time(&rig, removing(&rig, "osc2"), When::AtSample(at), Some(64));
    render_counted(&mut rig, 6);
    let outcome = outcomes_of(&mut rig, id);
    assert!(
        matches!(outcome[..], [(_, Outcome::Refused(_))]),
        "{outcome:?}"
    );
    // Its publication freed, the publisher stops waiting for it.
    rig.live.reclaim();
    let mut publisher = rig.live.publisher().lock().unwrap();
    assert!(publisher.adopt().is_none());
    assert!(!publisher.timed_waiting());
    assert!(!waiting(&publisher), "forgotten, the flag too");
    drop(publisher);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
}

#[test]
fn what_cannot_be_timed_is_refused_with_nothing_queued() {
    let rig = Rig::new(BASE);
    let mut publisher = rig.live.publisher().lock().unwrap();
    let refusal = |prepared, publisher: &mut Publisher| {
        let retain = (Box::new(|_: &mut RuntimeState| {}) as _, rig.live.port());
        let refused = publisher.publish_timed(prepared, When::AfterSamples(10), None, retain);
        refused
            .map(|_| ())
            .map_err(|refused| refused.error.to_string())
    };
    // Edits nothing.
    let empty = rig.live.change_on(&publisher).prepare().unwrap();
    let error = refusal(empty, &mut publisher).unwrap_err();
    assert!(error.contains("Only a structural edit"), "{error}");
    // A second while one waits.
    let mut change = rig.live.change_on(&publisher);
    change.remove("osc2");
    let prepared = change.prepare().unwrap();
    refusal(prepared, &mut publisher).unwrap();
    let mut change = rig.live.change_on(&publisher);
    change.remove("osc1");
    let error = refusal(change.prepare().unwrap(), &mut publisher).unwrap_err();
    assert!(error.contains("already waits"), "{error}");
    assert!(publisher.timed_waiting());
}
