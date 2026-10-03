//! `apply_edits` on a running invention, with the test thread rendering its
//! blocks in place of the audio thread.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::invention::builder::InventionBuilder;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::invention::runtime::RunningInvention;
use crate::rpc::{StructuralEdit, WrittenControl};
use crate::{ControlValue, Invention, ModuleRegistry};

const BASE: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "waveform": "sine", "frequency": 550.0 } },
        { "id": "spare", "type": "oscillator", "config": { "frequency": 3.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn doc(json: &str) -> Invention {
    Invention::from_json(json).unwrap()
}

fn start_with(registry: ModuleRegistry, json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry)
        .build(doc(json))
        .unwrap();
    start_manual(runtime)
}

fn start(json: &str) -> (RunningInvention, Pump) {
    start_with(ModuleRegistry::default(), json)
}

fn add(id: &str, module_type: &str, config: serde_json::Value) -> StructuralEdit {
    StructuralEdit::AddModule {
        id: id.into(),
        module_type: module_type.into(),
        config,
    }
}

fn remove(id: &str) -> StructuralEdit {
    StructuralEdit::RemoveModule { id: id.into() }
}

fn connect(from: &str, from_port: &str, to: &str, to_port: &str) -> StructuralEdit {
    StructuralEdit::Connect {
        from: from.into(),
        from_port: from_port.into(),
        to: to.into(),
        to_port: to_port.into(),
    }
}

fn set(module_id: &str, key: &str, value: ControlValue) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: module_id.into(),
        key: key.into(),
        value,
    }
}

fn number(value: f32) -> ControlValue {
    ControlValue::Number(value)
}

fn written(pairs: &[(&str, &str)]) -> Vec<WrittenControl> {
    pairs
        .iter()
        .map(|(module_id, key)| WrittenControl::new(*module_id, *key))
        .collect()
}

/// Records every event a runtime announces.
#[derive(Default)]
struct Events(Mutex<Vec<crate::RpcEventPayload>>);

impl crate::RpcEventSink for Events {
    fn emit(&self, event: crate::RpcEvent) {
        self.0.lock().unwrap().push(event.payload);
    }
}

impl Events {
    fn listen(running: &RunningInvention) -> Arc<Self> {
        let events = Arc::new(Self::default());
        running.set_event_sink(events.clone());
        events
    }

    fn control_changes(&self) -> Vec<(String, String, ControlValue)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                crate::RpcEventPayload::ControlChanged {
                    module_id,
                    key,
                    value,
                } => Some((module_id.clone(), key.clone(), value.clone())),
                _ => None,
            })
            .collect()
    }
}

fn change(module_id: &str, key: &str, value: f32) -> (String, String, ControlValue) {
    (module_id.into(), key.into(), number(value))
}

/// Publications made, and publications the audio thread has installed.
fn publications(running: &RunningInvention) -> (u64, u64) {
    let publisher = running.live.publisher().lock().unwrap();
    (publisher.generation(), publisher.applied())
}

fn config_of(running: &RunningInvention, id: &str) -> serde_json::Value {
    let document = running.document().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == id).unwrap();
    spec.config.clone()
}

#[test]
fn a_twenty_edit_batch_commits_as_one_publication_and_untouched_modules_keep_their_phase() {
    let (mut edited, edited_pump) = start(BASE);
    let (control, control_pump) = start(BASE);
    // Diverge osc2 at runtime in both, so a rebuild would be audible.
    for running in [&edited, &control] {
        running
            .set_control("osc2", "frequency", number(123.0))
            .unwrap();
    }
    assert_eq!(edited_pump.render(7), control_pump.render(7));
    let (generation, applied) = publications(&edited);

    // A chain of new voices nobody hears yet, tuned, with one added and
    // removed again, and the spare removed: 20 edits.
    let mut edits = Vec::new();
    for v in ["v0", "v1", "v2", "v3", "v4"] {
        edits.push(add(v, "oscillator", json!({ "frequency": 2.0 })));
    }
    for (from, to) in [("v0", "v1"), ("v1", "v2"), ("v2", "v3"), ("v3", "v4")] {
        edits.push(connect(from, "audio", to, "fm"));
    }
    for (index, v) in ["v0", "v1", "v2", "v3", "v4"].iter().enumerate() {
        edits.push(set(v, "frequency", number(100.0 + index as f32)));
    }
    edits.push(remove("spare"));
    edits.push(add("v5", "oscillator", json!({})));
    edits.push(remove("v5"));
    edits.push(add("v6", "oscillator", json!({})));
    edits.push(connect("v4", "audio", "v6", "fm"));
    edits.push(set("v6", "frequency", ControlValue::String("7".into())));
    assert_eq!(edits.len(), 20);

    let events = Events::listen(&edited);
    let report = edited.apply_edits(&edits).expect("the batch commits");
    assert_eq!(report.edit_count, 20);
    assert_eq!(report.added, ["v0", "v1", "v2", "v3", "v4", "v6"]);
    assert_eq!(report.removed, ["spare"]);
    assert!(report.rebuilt.is_empty());
    assert_eq!(
        report.controls_written,
        written(&[
            ("v0", "frequency"),
            ("v1", "frequency"),
            ("v2", "frequency"),
            ("v3", "frequency"),
            ("v4", "frequency"),
            ("v6", "frequency"),
        ])
    );
    assert!(report.controls_failed.is_empty());
    assert_eq!(
        (report.connections_added, report.connections_removed),
        (5, 0)
    );
    assert_eq!(report.untouched, 3);

    // One publication for the whole batch, installed at the next block.
    assert_eq!(publications(&edited), (generation + 1, applied));
    edited_pump.render(1);
    control_pump.render(1);
    assert_eq!(publications(&edited), (generation + 1, applied + 1));

    // The survivors kept their phase and runtime state: sample for sample
    // the same as a runtime the batch never touched.
    assert_eq!(edited_pump.render(20), control_pump.render(20));

    // Added modules were built from their final configs, which the
    // retained document holds, and each write was announced once.
    assert_eq!(edited.get_control("v6", "frequency").unwrap(), number(7.0));
    assert_eq!(config_of(&edited, "v3"), json!({ "frequency": 103 }));
    assert_eq!(events.control_changes().len(), 6);
    assert!(events
        .control_changes()
        .contains(&change("v6", "frequency", 7.0)));
    let ids: Vec<String> = edited
        .state
        .lock()
        .unwrap()
        .modules
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        ids,
        ["osc1", "osc2", "dac", "v0", "v1", "v2", "v3", "v4", "v6"]
    );
    assert_eq!(edited.state.lock().unwrap().connections.len(), 7);
}

#[test]
fn writes_to_a_module_a_later_edit_removes_or_replaces_are_dropped_with_it() {
    let (mut running, pump) = start(BASE);
    let events = Events::listen(&running);
    let report = running
        .apply_edits(&[
            set("osc2", "frequency", number(300.0)),
            remove("osc2"),
            set("osc1", "frequency", number(100.0)),
            set("osc1", "frequency", number(200.0)),
            set("spare", "frequency", number(9.0)),
            remove("spare"),
            add("spare", "oscillator", json!({ "frequency": 4.0 })),
            set("spare", "frequency", number(5.0)),
        ])
        .expect("the batch commits");

    assert_eq!(report.removed, ["osc2"]);
    assert_eq!(report.rebuilt, ["spare"]);
    assert_eq!(
        report.controls_written,
        written(&[("osc1", "frequency"), ("spare", "frequency")])
    );
    assert_eq!(report.untouched, 2);
    assert_eq!(
        events.control_changes(),
        [
            change("osc1", "frequency", 200.0),
            change("spare", "frequency", 5.0)
        ]
    );
    // The replacement was built from its final config, not the one added.
    assert_eq!(
        running.get_control("spare", "frequency").unwrap(),
        number(5.0)
    );
    pump.render(1);
}

#[test]
fn removing_and_adding_a_module_again_rebuilds_it_even_when_identical() {
    let (mut edited, edited_pump) = start(BASE);
    let (_control, control_pump) = start(BASE);
    assert_eq!(edited_pump.render(7), control_pump.render(7));

    let report = edited
        .apply_edits(&[
            remove("osc2"),
            add(
                "osc2",
                "oscillator",
                json!({ "waveform": "sine", "frequency": 550.0 }),
            ),
            connect("osc2", "audio", "dac", "audio"),
        ])
        .expect("the batch commits");
    assert_eq!(report.rebuilt, ["osc2"]);
    assert!(report.added.is_empty() && report.removed.is_empty());
    assert_eq!(
        (report.connections_added, report.connections_removed),
        (0, 0)
    );
    assert_eq!(report.untouched, 3);

    // A fresh instance restarts its phase, so the mix now differs.
    assert_ne!(edited_pump.render(4), control_pump.render(4));
}

#[test]
fn an_earlier_authored_write_does_not_count_as_a_change_of_the_batch() {
    // The standalone write updates the retained document, not the config
    // the module was built from. A batch that does not name the module
    // still commits, and leaves it untouched.
    let (mut running, pump) = start(BASE);
    running
        .set_control("spare", "frequency", number(7.0))
        .unwrap();
    let report = running
        .apply_edits(&[connect("osc1", "audio", "osc2", "fm")])
        .expect("the batch commits");
    assert_eq!(report.connections_added, 1);
    assert_eq!(report.untouched, 4);
    assert_eq!(config_of(&running, "spare")["frequency"], json!(7));
    pump.render(1);
}
