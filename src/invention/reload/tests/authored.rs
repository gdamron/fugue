//! Reloading after an authored control write: the runtime's stored configs
//! follow every authored write, so reload diffs against what the modules
//! play (FUG-289).

use serde_json::json;

use super::*;
use crate::rpc::StructuralEdit;
use crate::ControlWriteIntent;

fn number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

fn set(module_id: &str, key: &str, value: ControlValue) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: module_id.into(),
        key: key.into(),
        value,
    }
}

fn stored_config(running: &RunningInvention, id: &str) -> serde_json::Value {
    running.state.lock().unwrap().modules[id].config.clone()
}

/// Asserts the reload changed no module structurally: nothing added,
/// removed or rebuilt, and every module of BASE kept its instance.
fn assert_nothing_rebuilt(report: &ReloadReport) {
    assert!(report.added.is_empty(), "{report:?}");
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.unchanged, 3, "{report:?}");
}

#[test]
fn reloading_the_original_restores_an_authored_value_through_a_control_write() {
    let mut running = start(BASE);
    running
        .set_control("osc1", "frequency", number(330.0))
        .unwrap();

    let report = running.reload(doc(BASE)).expect("diff applies");

    // A control write on the surviving instance, not a rebuild.
    assert_eq!(report.controls_updated, ["osc1.frequency"]);
    assert_nothing_rebuilt(&report);
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(440.0)
    );
    assert_eq!(running.document(), Some(doc(BASE)));
}

#[test]
fn reloading_the_document_saved_after_an_authored_write_changes_nothing() {
    let mut running = start(BASE);
    // Fractional, so the f32's shortest decimal form must match on both
    // sides of the diff.
    running
        .set_control("osc1", "frequency", number(261.63))
        .unwrap();
    let saved = running.document().unwrap();

    let report = running.reload(saved).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_nothing_rebuilt(&report);
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(261.63)
    );
}

#[test]
fn a_performed_write_leaves_the_stored_configs_and_document_untouched() {
    let mut running = start(BASE);
    let (modules, document) = {
        let state = running.state.lock().unwrap();
        (state.modules.clone(), state.document.clone())
    };

    running
        .snapshot()
        .set_control_with_intent(
            "osc1",
            "frequency",
            number(330.0),
            ControlWriteIntent::Perform,
        )
        .unwrap();

    {
        let state = running.state.lock().unwrap();
        assert_eq!(state.modules, modules);
        assert_eq!(state.document, document);
    }
    // A live gesture is not authored, so reloading the original keeps it.
    let report = running.reload(doc(BASE)).expect("diff applies");
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_nothing_rebuilt(&report);
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(330.0)
    );
}

#[test]
fn reloading_the_original_restores_a_value_an_apply_edits_batch_wrote() {
    let mut running = start(BASE);
    let report = running
        .apply_edits(&[set("osc1", "frequency", number(330.0))])
        .expect("the batch commits");
    assert!(report.rebuilt.is_empty());
    assert_eq!(stored_config(&running, "osc1")["frequency"], json!(330));

    let report = running.reload(doc(BASE)).expect("diff applies");

    assert_eq!(report.controls_updated, ["osc1.frequency"]);
    assert_nothing_rebuilt(&report);
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(440.0)
    );
}

#[test]
fn reloading_the_document_saved_after_an_apply_edits_batch_changes_nothing() {
    // A survivor's write, a rebuilt module's and an added module's: each
    // stored config holds what the batch wrote, as the saved document does.
    let mut running = start(BASE);
    let report = running
        .apply_edits(&[
            set("osc1", "frequency", number(261.63)),
            StructuralEdit::RemoveModule { id: "osc2".into() },
            StructuralEdit::AddModule {
                id: "osc2".into(),
                module_type: "oscillator".into(),
                config: json!({ "frequency": 2.0 }),
            },
            set("osc2", "frequency", number(110.5)),
            StructuralEdit::Connect {
                from: "osc2".into(),
                from_port: "audio".into(),
                to: "dac".into(),
                to_port: "audio".into(),
            },
            StructuralEdit::AddModule {
                id: "osc3".into(),
                module_type: "oscillator".into(),
                config: json!({}),
            },
            set("osc3", "frequency", number(55.25)),
        ])
        .expect("the batch commits");
    assert_eq!(report.rebuilt, ["osc2"]);
    assert_eq!(report.added, ["osc3"]);
    let saved = running.document().unwrap();

    let report = running.reload(saved).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.added.is_empty(), "{report:?}");
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.unchanged, 4, "{report:?}");
    for (id, frequency) in [("osc1", 261.63), ("osc2", 110.5), ("osc3", 55.25)] {
        assert_eq!(
            running.get_control(id, "frequency").unwrap(),
            number(frequency)
        );
    }
}

#[test]
fn an_authored_write_to_a_key_the_file_omits_is_restored_by_a_rebuild() {
    // A known limit, pinned here rather than a goal: the authored write adds
    // `frequency` to a config that left it to the module's default. The
    // original file then lacks a key the stored config has, which reload
    // can only express by rebuilding the module (its phase restarts). A
    // control write back to the default would need a new kind of plan.
    let original = BASE.replace(
        r#""config": { "waveform": "sine", "frequency": 440.0 }"#,
        r#""config": { "waveform": "sine" }"#,
    );
    let mut running = start(&original);
    let default = running.get_control("osc1", "frequency").unwrap();
    assert_ne!(default, number(330.0));
    running
        .set_control("osc1", "frequency", number(330.0))
        .unwrap();

    let report = running.reload(doc(&original)).expect("diff applies");

    assert_eq!(report.swapped, ["osc1"]);
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.added.is_empty() && report.removed.is_empty());
    assert_eq!(report.unchanged, 2);
    assert_eq!(running.get_control("osc1", "frequency").unwrap(), default);
}
