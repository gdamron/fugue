//! `apply_edits` on a running invention, with the test thread rendering its
//! blocks in place of the audio thread.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::invention::builder::InventionBuilder;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::invention::runtime::RunningInvention;
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo};
use crate::rpc::{StructuralEdit, WrittenControl};
use crate::{ControlValue, Invention, ModuleRegistry};

mod commit;
mod config_refusals;
mod real_modules;
mod refusals;
mod scripted;

use scripted::{Scripted, SCRIPTED};

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

/// BASE with `module` (a module's JSON) added after the dac.
fn base_with(module: &str) -> String {
    BASE.replace(
        r#"{ "id": "dac", "type": "dac" }"#,
        &format!(r#"{{ "id": "dac", "type": "dac" }}, {module}"#),
    )
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

    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
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

/// Everything a refused batch must leave as it was.
#[derive(Debug, PartialEq)]
struct Observed {
    modules: indexmap::IndexMap<String, RuntimeModuleInfo>,
    connections: Vec<RuntimeConnectionInfo>,
    document: Option<Invention>,
    surfaces: Vec<String>,
    ports: Vec<String>,
    generation: u64,
}

fn observe(running: &RunningInvention) -> Observed {
    let state = running.state.lock().unwrap();
    Observed {
        modules: state.modules.clone(),
        connections: state.connections.clone(),
        document: state.document.clone(),
        surfaces: running
            .control_surfaces
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect(),
        ports: running
            .module_ports
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect(),
        generation: running.live.generation(),
    }
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
        edits.push(connect(from, "audio", to, "frequency_mod"));
    }
    for (index, v) in ["v0", "v1", "v2", "v3", "v4"].iter().enumerate() {
        edits.push(set(v, "frequency", number(100.0 + index as f32)));
    }
    edits.push(remove("spare"));
    edits.push(add("v5", "oscillator", json!({})));
    edits.push(remove("v5"));
    edits.push(add("v6", "oscillator", json!({})));
    edits.push(connect("v4", "audio", "v6", "frequency_mod"));
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
fn control_only_edits_write_each_value_once_in_order_and_announce_the_final_values() {
    let scripted = Scripted::default();
    let base = base_with(r#"{ "id": "counter", "type": "scripted", "config": { "level": 0.1 } }"#);
    let (mut running, pump) = start_with(scripted.registry(), &base);
    let events = Events::listen(&running);
    let generation = running.live.generation();

    let report = running
        .apply_edits(&[
            set("counter", "level", number(0.25)),
            set("osc1", "frequency", number(220.0)),
            set("counter", "level", number(0.5)),
        ])
        .expect("the batch commits");

    // Each write is made once, in order: first on the throwaway copy the
    // batch is checked against, then on the running module. The plan's own
    // config-as-control updates are not applied besides.
    let level = |value: f32| ("level".to_string(), number(value));
    assert_eq!(
        scripted.writes(),
        [level(0.25), level(0.5), level(0.25), level(0.5)]
    );
    pump.block();
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(220.0)
    );
    assert_eq!(
        events.control_changes(),
        [
            change("counter", "level", 0.5),
            change("osc1", "frequency", 220.0)
        ]
    );
    assert_eq!(
        report.controls_written,
        written(&[("counter", "level"), ("osc1", "frequency")])
    );
    assert_eq!(report.untouched, 5);
    assert!(report.added.is_empty() && report.rebuilt.is_empty());

    // Nothing structural changed, so nothing was published; the document
    // holds the values as authored.
    assert_eq!(running.live.generation(), generation);
    assert_eq!(config_of(&running, "counter")["level"], json!(0.5));
    assert_eq!(config_of(&running, "osc1")["frequency"], json!(220));
    pump.render(1);
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
fn an_added_module_is_built_for_real_once_and_written_through_its_setter() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);

    // Checked by `describe` and by the validation build, both thrown away,
    // then built once for real by the commit.
    running
        .apply_edits(&[add("a", SCRIPTED, json!({ "level": 0.1 }))])
        .expect("the batch commits");
    assert_eq!(scripted.builds(), 3);
    assert!(scripted.writes().is_empty());

    // One check, a validation build of both modules, and the commit's
    // build. The write goes through the module's setter, as it would for a
    // module whose control key is not its config key.
    running
        .apply_edits(&[
            add("b", SCRIPTED, json!({ "level": 0.1 })),
            set("b", "level", number(0.5)),
        ])
        .expect("the batch commits");
    assert_eq!(scripted.builds(), 7);
    assert_eq!(running.get_control("b", "level").unwrap(), number(0.5));
    // Once on the described instance the batch is checked against, once on
    // the instance built for real.
    assert_eq!(
        scripted.writes(),
        [
            ("level".to_string(), number(0.5)),
            ("level".to_string(), number(0.5))
        ]
    );
    pump.render(1);
}

#[test]
fn an_earlier_authored_write_does_not_count_as_a_change_of_the_batch() {
    // The standalone write updates the retained document and the module's
    // stored config. A batch that does not name the module still commits,
    // and leaves it untouched.
    let (mut running, pump) = start(BASE);
    running
        .set_control("spare", "frequency", number(7.0))
        .unwrap();
    let report = running
        .apply_edits(&[connect("osc1", "audio", "osc2", "frequency_mod")])
        .expect("the batch commits");
    assert_eq!(report.connections_added, 1);
    assert_eq!(report.untouched, 4);
    assert_eq!(config_of(&running, "spare")["frequency"], json!(7));
    pump.render(1);
}
