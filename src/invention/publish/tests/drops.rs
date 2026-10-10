//! Nothing a change built is dropped while the publisher is locked, so a
//! sink finalizing its output never holds up another edit.

use super::*;

/// A rig whose probes record whether the publisher was locked when they
/// dropped.
fn watched_rig() -> (Rig, DropProbeFactory) {
    let probes = DropProbeFactory::default();
    let mut rig = Rig::new(BASE);
    rig.registry.register(probes.clone());
    rig.adopt_registry();
    probes.watch(rig.live.publisher().clone());
    (rig, probes)
}

#[test]
fn a_refused_change_drops_off_the_publisher_lock() {
    let (rig, probes) = watched_rig();
    let mut stale = rig.live.begin();
    stale.upsert(
        "probe",
        rig.build("probe", DROP_PROBE, serde_json::json!({})),
    );
    let stale = stale.prepare().unwrap();
    rig.live.remove_module("osc2").unwrap();

    assert!(matches!(
        rig.live.commit(stale),
        Err(GraphCommandError::TopologyMoved)
    ));
    assert_eq!(probes.dropped_under_lock(), [false]);
}

#[test]
fn a_failed_edit_drops_off_the_publisher_lock() {
    let (rig, probes) = watched_rig();
    let probe = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    let failed = rig.live.edit(|change| {
        change.upsert("probe", probe);
        change.connect(edge("osc1", "nope", "probe", "audio"))
    });
    assert!(matches!(failed, Err(GraphCommandError::InvalidPort(_))));

    // A schedule that cannot resolve fails while the change is prepared.
    let probe = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    let sched = rig.build(
        "sched",
        "control_scheduler",
        serde_json::json!({
            "schedule": [{ "at_step": 0, "module": "missing", "control": "frequency", "value": 1.0 }]
        }),
    );
    let failed = rig.live.edit(|change| {
        change.upsert("probe", probe);
        change.upsert("sched", sched);
        Ok(())
    });
    assert!(matches!(
        failed,
        Err(GraphCommandError::ModuleBuildFailed(_))
    ));
    assert_eq!(probes.dropped_under_lock(), [false, false]);
}

#[test]
fn a_module_a_queued_edit_removes_drops_off_the_publisher_lock() {
    let (mut rig, probes) = watched_rig();
    let probe = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("probe", probe);
            Ok(())
        })
        .unwrap();

    // Both edits install at the next block; the probe leaves with the
    // second's retired publication, freed by the reclaimer.
    rig.live.remove_module("probe").unwrap();
    assert!(probes.dropped_under_lock().is_empty());
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
    rig.live.reclaim();
    assert_eq!(probes.dropped_under_lock(), [false]);
}

#[test]
fn a_swap_onto_a_missing_module_drops_off_the_publisher_lock() {
    let (rig, probes) = watched_rig();
    assert!(matches!(
        rig.live.swap_module(
            SAMPLE_RATE,
            "missing",
            DROP_PROBE,
            &serde_json::json!({}),
            true,
        ),
        Err(GraphCommandError::UnknownModule(_))
    ));
    assert_eq!(probes.dropped_under_lock(), [false]);
}

#[test]
fn a_module_an_edit_displaces_drops_off_the_publisher_lock() {
    let (rig, probes) = watched_rig();
    let replaced = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    let kept = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("probe", replaced);
            change.upsert("probe", kept);
            Ok(())
        })
        .unwrap();
    assert_eq!(probes.dropped_under_lock(), [false]);

    let removed = rig.build("probe2", DROP_PROBE, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("probe2", removed);
            change.remove("probe2");
            Ok(())
        })
        .unwrap();
    assert_eq!(probes.dropped_under_lock(), [false, false]);
}
