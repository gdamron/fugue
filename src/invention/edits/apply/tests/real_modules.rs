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

/// Waits, up to two seconds, for a recorder's writer thread to create
/// `path`: it opens the file once the recorder first processes audio.
fn appears(path: &std::path::Path) -> bool {
    for _ in 0..200 {
        if path.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
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
    assert!(appears(&path));
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
    assert!(appears(&path));
}

#[test]
fn a_control_written_to_an_added_module_is_made_through_its_setter() {
    // An oscillator's `waveform` is both its config key and its control.
    let (mut running, pump) = start(BASE);
    let report = running
        .apply_edits(&[
            add("lead", "oscillator", json!({ "frequency": 220.0 })),
            set("lead", "waveform", text("square")),
        ])
        .expect("the batch commits");
    assert!(report.controls_failed.is_empty());
    assert_eq!(
        running.get_control("lead", "waveform").unwrap(),
        text("square")
    );
    pump.render(1);
    assert_eq!(
        running.get_control("lead", "waveform").unwrap(),
        text("square")
    );
}

#[test]
fn a_control_written_to_a_replaced_module_is_made_through_its_setter() {
    let (mut running, _pump) = start(BASE);
    running
        .apply_edits(&[
            remove("spare"),
            add("spare", "oscillator", json!({ "frequency": 330.0 })),
            set("spare", "waveform", text("sawtooth")),
        ])
        .expect("the batch commits");
    assert_eq!(
        running.get_control("spare", "waveform").unwrap(),
        text("sawtooth")
    );
}

const TARGETS_OSC1: &str =
    r#"[{ "at_step": 0, "module": "osc1", "control": "frequency", "value": 220.0 }]"#;

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
                text(r#"[{ "at_step": 0, "module": "ghost", "control": "x", "value": 1.0 }]"#),
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

/// A melody's degrees and weights as a module rebuilt from its retained
/// document config has them, against the live module.
fn assert_document_rebuilds_the_live_melody(running: &RunningInvention) {
    let document = running.document().unwrap();
    let config = &document
        .modules
        .iter()
        .find(|spec| spec.id == "tune")
        .unwrap()
        .config;
    let rebuilt = crate::ModuleRegistry::default()
        .build("melody", SAMPLE_RATE, config)
        .unwrap()
        .control_surface
        .unwrap();
    let count = rebuilt.get_control("degree_count").unwrap();
    assert_eq!(
        Some(count.clone()),
        running.get_control("tune", "degree_count").ok()
    );
    let ControlValue::Number(count) = count else {
        unreachable!()
    };
    for i in 0..count as usize {
        for key in [format!("degree.{i}"), format!("note_weight.{i}")] {
            assert_eq!(
                rebuilt.get_control(&key).ok(),
                running.get_control("tune", &key).ok(),
                "{key} in {config}"
            );
        }
    }
}

#[test]
fn a_saved_melody_rebuilds_what_shrinking_and_growing_left_playing() {
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "tune".into(),
        module_type: "melody".into(),
        config: json!({ "degrees": [0, 2, 4, 5, 7, 9, 11], "note_weights": [4, 1, 2] }),
    });
    let (mut running, _pump) = start_doc(document);
    running
        .set_control("tune", "degree.6", number(1.0))
        .unwrap();
    running
        .set_control("tune", "degree_count", number(3.0))
        .unwrap();
    running
        .apply_edits(&[
            set("tune", "degree.1", number(3.0)),
            set("tune", "degree_count", number(10.0)),
        ])
        .expect("the batch commits");
    running
        .set_control("tune", "note_weight.8", number(5.0))
        .unwrap();
    running
        .set_control("tune", "degree_count", number(9.0))
        .unwrap();
    assert_document_rebuilds_the_live_melody(&running);
}

/// BASE with a melody of `scale`.
fn with_melody(scale: serde_json::Value) -> Invention {
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "tune".into(),
        module_type: "melody".into(),
        config: json!({ "degrees": scale }),
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
            add("second", "melody", json!({ "degrees": [0, 1, 2] })),
            set("second", "degree_count", number(7.0)),
            set("second", "degree.6", number(12.0)),
        ])
        .expect("the batch commits");
    assert_eq!(
        running.get_control("second", "degree.6").unwrap(),
        number(12.0)
    );
}

#[test]
fn a_batch_sees_controls_an_earlier_authored_write_added() {
    // The melody's factory does not read a recorded `degree_count`; the
    // copy a batch checks against still has the degrees the authored write
    // added.
    let (mut running, _pump) = start_doc(with_melody(json!([0, 1, 2])));
    running
        .set_control("tune", "degree_count", number(7.0))
        .unwrap();
    running
        .apply_edits(&[set("tune", "degree.6", number(12.0))])
        .expect("degree 6 exists since the authored write");
    assert_eq!(
        running.get_control("tune", "degree.6").unwrap(),
        number(12.0)
    );
}

#[test]
fn every_schedule_a_batch_writes_is_checked_against_the_graph() {
    // An earlier schedule naming a module that does not exist is refused at
    // its edit, though a later one overwrites it.
    let ghost = r#"[{ "at_step": 0, "module": "ghost", "control": "x", "value": 1.0 }]"#;
    let (mut running, _pump) = start(BASE);
    let error = running
        .apply_edits(&[
            add("auto", "control_scheduler", json!({})),
            set("auto", "schedule", text(ghost)),
            set("auto", "schedule", text("[]")),
        ])
        .expect_err("the first schedule's target does not exist");
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    assert_eq!(error.edit.as_ref().unwrap().index, 1);
}

#[test]
fn a_control_is_settled_by_its_last_write() {
    // A live gesture shrank the melody below its authored count, so the
    // batch's first write to degree 6 fails when made; the count it then
    // restores lets the last write land. The control is written, not failed.
    let (mut running, _pump) = start_doc(with_melody(json!([0, 1, 2, 3, 4, 5, 6])));
    running
        .snapshot()
        .set_control_with_intent(
            "tune",
            "degree_count",
            number(3.0),
            crate::ControlWriteIntent::Perform,
        )
        .unwrap();
    let report = running
        .apply_edits(&[
            set("tune", "degree.6", number(12.0)),
            set("tune", "degree_count", number(7.0)),
            set("tune", "degree.6", number(13.0)),
        ])
        .expect("the batch commits");
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert!(report
        .controls_written
        .iter()
        .any(|written| written.key == "degree.6"));
    assert_eq!(
        running.get_control("tune", "degree.6").unwrap(),
        number(13.0)
    );
    let document = running.document().unwrap();
    let tune = document
        .modules
        .iter()
        .find(|spec| spec.id == "tune")
        .unwrap();
    assert_eq!(tune.config["degree.6"], json!(13));
}

/// Writes a short silent mono WAV named `name` in `dir`.
fn silent_wav(dir: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..64 {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    path
}

#[test]
fn a_sample_that_does_not_load_is_reported_when_written_and_the_batch_commits() {
    // What is left to the write itself, not to the module's rules, fails
    // when the write is made, as for a standalone write.
    let dir = tempfile::tempdir().unwrap();
    let sample = silent_wav(&dir, "kick.wav");
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "kit".into(),
        module_type: "sample_kit".into(),
        config: json!({ "samples": [{ "key": "kick", "asset": sample }] }),
    });
    let (mut running, pump) = start_doc(document);
    let report = running
        .apply_edits(&[
            set("osc1", "frequency", number(220.0)),
            set("kit", "asset.0", text("/no/such/sample.wav")),
        ])
        .expect("the batch commits");
    assert_eq!(report.controls_failed.len(), 1, "{report:?}");
    assert_eq!(report.controls_failed[0].edit_index, 1);
    pump.block();
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(220.0)
    );
}

#[test]
fn a_schedule_targeting_its_own_scheduler_is_refused_though_overwritten() {
    let own = r#"[{ "at_step": 0, "module": "auto", "control": "step", "value": 1.0 }]"#;
    let (mut running, _pump) = start(BASE);
    let error = running
        .apply_edits(&[
            add("auto", "control_scheduler", json!({})),
            set("auto", "schedule", text(own)),
            set("auto", "schedule", text("[]")),
        ])
        .expect_err("a scheduler cannot target itself");
    assert_eq!(error.code, RpcErrorCode::InvalidEdit);
    assert_eq!(error.edit.as_ref().unwrap().index, 1);
}

#[test]
fn a_sample_can_be_replaced_when_the_one_it_loaded_no_longer_builds() {
    // The running module's authored config no longer builds (as when the
    // package its sample came from has been removed since it loaded); its
    // write is checked on the running module instead.
    let dir = tempfile::tempdir().unwrap();
    let old = silent_wav(&dir, "old.wav");
    let new = silent_wav(&dir, "new.wav");
    let mut document = doc(BASE);
    document.modules.push(crate::ModuleSpec {
        id: "smp".into(),
        module_type: "sample_player".into(),
        config: json!({ "asset": old }),
    });
    let (mut running, _pump) = start_doc(document);
    running.state.lock().unwrap().document_write_control(
        "smp",
        "asset",
        &text(&dir.path().join("gone.wav").to_string_lossy()),
    );

    let report = running
        .apply_edits(&[set("smp", "asset", text(&new.to_string_lossy()))])
        .expect("the new sample is valid");
    assert!(report.controls_failed.is_empty(), "{report:?}");
}

#[test]
fn a_development_alias_hidden_by_a_later_count_is_not_refused() {
    // Writing a degree through a development's alias, then shrinking the
    // count through another, is valid in order.
    let document: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "developments": [{
            "name": "tuned",
            "definition": {
                "version": "1.0.0",
                "modules": [{ "id": "m", "type": "melody", "config": { "degrees": [0, 1, 2, 3, 4, 5, 6] } }],
                "connections": [],
                "outputs": [{ "name": "freq", "from": "m", "from_port": "frequency" }],
                "controls": [
                    { "name": "deg6", "module": "m", "control": "degree.6" },
                    { "name": "count", "module": "m", "control": "degree_count" }
                ]
            }
        }],
        "modules": [{ "id": "t", "type": "tuned" }, { "id": "dac", "type": "dac" }],
        "connections": []
    }))
    .unwrap();
    let (mut running, _pump) = start_doc(document);
    running
        .apply_edits(&[
            set("t", "deg6", number(12.0)),
            set("t", "count", number(3.0)),
        ])
        .expect("each write is valid where it stands");
    assert_eq!(running.get_control("t", "count").unwrap(), number(3.0));
}

#[test]
fn an_added_module_recovers_from_an_overwritten_sample_failure() {
    let dir = tempfile::tempdir().unwrap();
    let kick = silent_wav(&dir, "kick.wav");
    let snare = silent_wav(&dir, "snare.wav");
    let (mut running, _pump) = start(BASE);
    let report = running
        .apply_edits(&[
            add(
                "kit",
                "sample_kit",
                json!({ "samples": [{ "key": "kick", "asset": kick }] }),
            ),
            set("kit", "asset.0", text("/no/such/sample.wav")),
            set("kit", "asset.0", text(&snare.to_string_lossy())),
        ])
        .expect("the last write loads");
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(
        running.get_control("kit", "asset.0").unwrap(),
        text(&snare.to_string_lossy())
    );
}
#[test]
fn the_document_and_events_carry_what_a_write_left_after_later_writes() {
    // A shrinking count hides a degree written before it and the regrowing
    // count shows it as written, so the document and the event, which carry
    // the written value, match what the module plays.
    let (mut running, _pump) = start_doc(with_melody(json!([0, 1, 2, 3, 4, 5, 6])));
    let events = super::Events::listen(&running);
    running
        .apply_edits(&[
            set("tune", "degree.6", number(12.0)),
            set("tune", "degree_count", number(3.0)),
            set("tune", "degree_count", number(7.0)),
        ])
        .expect("the batch commits");
    let live = running.get_control("tune", "degree.6").unwrap();
    let document = running.document().unwrap();
    let tune = document
        .modules
        .iter()
        .find(|spec| spec.id == "tune")
        .unwrap();
    let ControlValue::Number(live_number) = live.clone() else {
        unreachable!()
    };
    assert_eq!(
        tune.config["degree.6"].as_f64(),
        Some(f64::from(live_number))
    );
    let announced = events
        .control_changes()
        .into_iter()
        .find(|(module, key, _)| module == "tune" && key == "degree.6")
        .map(|(_, _, value)| value);
    assert_eq!(announced, Some(live));
}
