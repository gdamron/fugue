//! Nothing a change built is dropped while the publisher is locked, so a
//! sink finalizing its output never holds up another edit.

use super::*;

/// A rig whose probes record whether the publisher was locked when they
/// dropped.
fn watched_rig() -> (Rig, DropProbeFactory) {
    let probes = DropProbeFactory::default();
    let mut rig = Rig::new(BASE);
    rig.registry.register(probes.clone());
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
            "schedule": [{ "at": 0, "module": "missing", "control": "frequency", "value": 1.0 }]
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
fn a_superseded_publication_drops_off_the_publisher_lock() {
    let (mut rig, probes) = watched_rig();
    let probe = rig.build("probe", DROP_PROBE, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("probe", probe);
            Ok(())
        })
        .unwrap();

    // The audio thread never takes the first publication, so the second
    // folds it in, and the probe it no longer needs is superseded.
    rig.live.remove_module("probe").unwrap();
    assert_eq!(probes.dropped_under_lock(), [false]);
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
}
