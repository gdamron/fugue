//! Batches over modules whose config and controls differ in shape, or whose
//! build reaches past the graph: recording sinks, schedulers, asset-backed
//! configs and nested developments.

use serde_json::json;

use super::{add, doc, number, remove, set, start, BASE};
use crate::alloc_counter::allocator_events;
use crate::invention::builder::InventionBuilder;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::invention::runtime::RunningInvention;
use crate::rpc::RpcErrorCode;
use crate::{ControlValue, Invention};

fn text(value: &str) -> ControlValue {
    ControlValue::String(value.to_string())
}

fn start_doc(document: Invention) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    start_manual(runtime)
}

/// BASE with a recorder fed by osc1, writing to `path`.
fn recording(path: &std::path::Path) -> Invention {
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "rec".into(),
        module_type: "audio_file_sink".into(),
        config: json!({ "path": path }),
    });
    document.connections.push(crate::Connection {
        from: "osc1".into(),
        to: "rec".into(),
        from_port: Some("audio".into()),
        to_port: Some("audio".into()),
    });
    document
}

#[test]
fn checking_a_batch_never_builds_a_recording_sink_over_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("take.wav");
    let (mut running, pump) = start_doc(recording(&path));
    pump.render(4);
    // The recorder keeps writing through its open file. Were the batch's
    // check to build the document's recorder again, it would create the
    // file anew (truncating it, had it still been there).
    std::fs::remove_file(&path).unwrap();

    running
        .apply_edits(&[set("spare", "frequency", number(5.0))])
        .expect("the batch commits");
    pump.render(1);
    assert!(
        !path.exists(),
        "the batch rebuilt the recorder over its file"
    );
}

#[test]
fn a_refused_batch_never_opens_the_file_of_a_recorder_it_adds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.wav");
    let (mut running, pump) = start(BASE);

    let error = running
        .apply_edits(&[
            add("rec", "audio_file_sink", json!({ "path": path })),
            remove("no_such_module"),
        ])
        .expect_err("the second edit fails");
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    assert!(
        !path.exists(),
        "checking the batch opened the recorder's file"
    );

    // Committed, the recorder is built for real.
    running
        .apply_edits(&[add("rec", "audio_file_sink", json!({ "path": path }))])
        .expect("the batch commits");
    pump.render(1);
    assert!(path.exists());
}

#[test]
fn a_control_written_to_an_added_module_is_made_through_its_setter() {
    // An oscillator is built from `oscillator_type`; its control is `type`.
    let (mut running, pump) = start(BASE);
    let report = running
        .apply_edits(&[
            add("lead", "oscillator", json!({ "frequency": 220.0 })),
            set("lead", "type", text("square")),
        ])
        .expect("the batch commits");
    assert!(report.controls_failed.is_empty());
    assert_eq!(running.get_control("lead", "type").unwrap(), text("square"));
    pump.render(1);
    assert_eq!(running.get_control("lead", "type").unwrap(), text("square"));
}

#[test]
fn a_control_written_to_a_replaced_module_is_made_through_its_setter() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[
            remove("spare"),
            add("spare", "oscillator", json!({ "frequency": 330.0 })),
            set("spare", "type", text("sawtooth")),
        ])
        .expect("the batch commits");
    assert_eq!(
        running.get_control("spare", "type").unwrap(),
        text("sawtooth")
    );
}

const TARGETS_OSC1: &str =
    r#"[{ "at": 0, "module": "osc1", "control": "frequency", "value": 220.0 }]"#;

#[test]
fn a_scheduler_added_in_a_batch_takes_a_schedule_written_in_it() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[
            add("auto", "control_scheduler", json!({})),
            set("auto", "schedule", text(TARGETS_OSC1)),
        ])
        .expect("the batch commits");
    let schedule = running.get_control("auto", "schedule").unwrap();
    assert!(
        schedule.as_string().unwrap().contains("osc1"),
        "{schedule:?}"
    );

    // One targeting a module nothing has is still refused at its edit.
    let error = running
        .apply_edits(&[
            add("auto2", "control_scheduler", json!({})),
            set(
                "auto2",
                "schedule",
                text(r#"[{ "at": 0, "module": "ghost", "control": "x", "value": 1.0 }]"#),
            ),
        ])
        .expect_err("the target does not exist");
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
}

#[test]
fn a_scheduler_added_with_its_schedule_starts_without_allocating() {
    // The schedule is the scheduler's when it is prepared, so the audio
    // thread has nothing new to adopt after the swap.
    let (mut running, pump) = start(BASE);
    pump.render(2);
    running
        .apply_edits(&[
            add("auto", "control_scheduler", json!({})),
            set("auto", "schedule", text(TARGETS_OSC1)),
        ])
        .expect("the batch commits");
    for block in 0..2 {
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "block {block}");
    }
    let schedule = running.get_control("auto", "schedule").unwrap();
    assert!(
        schedule.as_string().unwrap().contains("osc1"),
        "{schedule:?}"
    );
}

#[test]
fn a_schedule_written_to_a_running_scheduler_commits() {
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "auto".into(),
        module_type: "control_scheduler".into(),
        config: json!({ "schedule": [] }),
    });
    let (mut running, _pump) = start_doc(document);

    for schedule in [TARGETS_OSC1, "[]"] {
        let report = running
            .apply_edits(&[set("auto", "schedule", text(schedule))])
            .expect("the batch commits");
        assert!(report.rebuilt.is_empty());
        assert!(report.controls_failed.is_empty());
    }
    assert_eq!(running.get_control("auto", "schedule").unwrap(), text("[]"));
}

#[test]
fn an_added_module_resolves_its_assets_against_the_running_document() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("automation.json"), TARGETS_OSC1).unwrap();
    let mut document = doc(BASE);
    document.assets.insert(
        "automation".into(),
        crate::invention::format::AssetSpec {
            path: "automation.json".into(),
        },
    );
    let root = dir.path().join("root.json");
    std::fs::write(&root, document.to_json().unwrap()).unwrap();
    let (mut running, _pump) = start_doc(Invention::from_file(&root.to_string_lossy()).unwrap());

    running
        .apply_edits(&[add(
            "auto",
            "control_scheduler",
            json!({ "schedule": { "$asset": "automation" } }),
        )])
        .expect("the batch commits");
    let schedule = running.get_control("auto", "schedule").unwrap();
    assert!(
        schedule.as_string().unwrap().contains("osc1"),
        "{schedule:?}"
    );
    // The document keeps the reference as authored.
    let authored = running.document().unwrap();
    let spec = authored.modules.iter().find(|spec| spec.id == "auto");
    assert_eq!(
        spec.unwrap().config,
        json!({ "schedule": { "$asset": "automation" } })
    );
}

#[test]
fn a_batch_checks_nested_developments_as_loaded_not_as_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("inner.json");
    std::fs::write(
        &inner,
        json!({
            "version": "1.0.0",
            "modules": [{ "id": "osc", "type": "oscillator" }],
            "connections": [],
            "outputs": [{ "name": "audio", "from": "osc", "from_port": "audio" }]
        })
        .to_string(),
    )
    .unwrap();
    let document: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{
            "name": "outer_voice",
            "definition": {
                "version": "1.0.0",
                "developments": [{ "name": "inner_voice", "path": inner }],
                "modules": [{ "id": "voice", "type": "inner_voice" }],
                "connections": [],
                "outputs": [{ "name": "audio", "from": "voice", "from_port": "audio" }]
            }
        }],
        "modules": [
            { "id": "pad", "type": "outer_voice" },
            { "id": "spare", "type": "oscillator" },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [{ "from": "pad", "from_port": "audio", "to": "dac", "to_port": "audio" }]
    }))
    .unwrap();
    let (mut running, _pump) = start_doc(document);
    std::fs::remove_file(&inner).unwrap();

    running
        .apply_edits(&[set("spare", "frequency", number(5.0))])
        .expect("the batch commits");
    // A new instance builds from the definitions loaded with the document.
    running
        .apply_edits(&[add("pad2", "outer_voice", serde_json::Value::Null)])
        .expect("the batch commits");
}

/// BASE with a melody of `scale`.
fn with_melody(scale: serde_json::Value) -> Invention {
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "tune".into(),
        module_type: "melody".into(),
        config: json!({ "scale_degrees": scale }),
    });
    document
}

#[test]
fn a_write_to_a_control_an_earlier_edit_removed_is_refused_at_its_index() {
    let (mut running, _pump) = start_doc(with_melody(json!([0, 1, 2, 3, 4, 5, 6])));
    let before = running.document().unwrap();
    let error = running
        .apply_edits(&[
            set("tune", "degree_count", number(3.0)),
            set("tune", "degree.6", number(12.0)),
        ])
        .expect_err("degree 6 is gone once the count is 3");
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    assert_eq!(error.edit.as_ref().unwrap().index, 1);
    // Nothing changed: not the count, not the document.
    assert_eq!(
        running.get_control("tune", "degree_count").unwrap(),
        number(7.0)
    );
    assert_eq!(running.document().unwrap(), before);
}

#[test]
fn a_write_to_a_control_an_earlier_edit_added_commits() {
    let (mut running, _pump) = start_doc(with_melody(json!([0, 1, 2])));
    let report = running
        .apply_edits(&[
            set("tune", "degree_count", number(7.0)),
            set("tune", "degree.6", number(12.0)),
        ])
        .expect("degree 6 exists once the count is 7");
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(
        running.get_control("tune", "degree.6").unwrap(),
        number(12.0)
    );
}

#[test]
fn writes_land_in_batch_order() {
    // Valid in order: the degree is written while it exists, then hidden or
    // removed by the count. Applying the final values in another order would
    // fail the degree's write.
    let (mut running, _pump) = start_doc(with_melody(json!([0, 1, 2, 3, 4, 5, 6])));
    let report = running
        .apply_edits(&[
            set("tune", "degree_count", number(9.0)),
            set("tune", "degree.8", number(12.0)),
            set("tune", "degree_count", number(3.0)),
        ])
        .expect("each write is valid where it stands");
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(
        running.get_control("tune", "degree_count").unwrap(),
        number(3.0)
    );

    // A module the batch adds takes its writes in order too.
    running
        .apply_edits(&[
            add("second", "melody", json!({ "scale_degrees": [0, 1, 2] })),
            set("second", "degree_count", number(7.0)),
            set("second", "degree.6", number(12.0)),
        ])
        .expect("the batch commits");
    assert_eq!(
        running.get_control("second", "degree.6").unwrap(),
        number(12.0)
    );
}
