//! An edit racing a reload never commits a module built from a development
//! definition the reload superseded.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use super::*;
use crate::invention::orchestration::RuntimeController;

/// A pause one live build of a [`Gate`] takes: it signals the build on the
/// first end, then waits to be released on the second.
type Pause = (SyncSender<()>, Receiver<()>);

/// Long enough for any build or reload here; a hang fails instead.
const WAIT: Duration = Duration::from_secs(10);

/// A port-less module that records the `tag` of each live build and can
/// pause one between building and committing, standing in for a slow build
/// on a script's thread. Placed inside a development's definition, it shows
/// which definition each build of the development came from.
#[derive(Clone, Default)]
struct Gate {
    built: Arc<Mutex<Vec<String>>>,
    pauses: Arc<Mutex<VecDeque<Pause>>>,
}

impl Gate {
    /// Pauses the next live build that has no pause yet. Returns the end the
    /// build signals on and the end that releases it.
    fn pause(&self) -> (Receiver<()>, SyncSender<()>) {
        let (built_tx, built_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        self.pauses
            .lock()
            .unwrap()
            .push_back((built_tx, release_rx));
        (built_rx, release_tx)
    }

    fn built(&self) -> Vec<String> {
        self.built.lock().unwrap().clone()
    }
}

impl crate::ModuleFactory for Gate {
    fn type_id(&self) -> &'static str {
        "gate"
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
        let tag = config["tag"].as_str().unwrap_or_default().to_string();
        self.built.lock().unwrap().push(tag);
        let pause = self.pauses.lock().unwrap().pop_front();
        if let Some((built, release)) = pause {
            built.send(()).unwrap();
            release
                .recv_timeout(WAIT)
                .expect("the test releases the build");
        }
        self.build_for_inspection(SAMPLE_RATE, config)
    }

    fn build_for_inspection(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
        let module = crate::ModuleRegistry::default()
            .build("code", SAMPLE_RATE, &serde_json::json!({}))?
            .module;
        Ok(crate::ModuleBuildResult {
            module,
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

/// A document declaring the `voice` development, unused, whose gate is
/// tagged `tag`. The `v2` definition also has an `aux` output.
fn document(tag: &str) -> Invention {
    let aux = if tag == "v2" {
        r#", { "name": "aux", "from": "o", "from_port": "audio" }"#
    } else {
        ""
    };
    doc(&format!(
        r#"{{
            "version": "1.0.0",
            "developments": [
                {{
                    "name": "voice",
                    "definition": {{
                        "modules": [
                            {{ "id": "gate", "type": "gate", "config": {{ "tag": "{tag}" }} }},
                            {{ "id": "o", "type": "oscillator" }}
                        ],
                        "connections": [],
                        "outputs": [ {{ "name": "audio", "from": "o", "from_port": "audio" }}{aux} ]
                    }}
                }}
            ],
            "modules": [
                {{ "id": "osc1", "type": "oscillator" }},
                {{ "id": "dac", "type": "dac" }}
            ],
            "connections": [
                {{ "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" }}
            ]
        }}"#
    ))
}

fn start_gated() -> (RunningInvention, Pump, Gate) {
    let gate = Gate::default();
    let mut registry = crate::ModuleRegistry::default();
    registry.register(gate.clone());
    let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry)
        .build(document("v1"))
        .unwrap();
    let (running, pump) = start_manual(runtime);
    (running, pump, gate)
}

/// Adds a `voice` named `late` through `controller` on its own thread, as
/// a script would.
fn add_voice(
    controller: RuntimeController,
) -> std::thread::JoinHandle<Result<(), GraphCommandError>> {
    std::thread::spawn(move || {
        controller
            .add_module("late", "voice", &serde_json::json!({}))
            .map(drop)
    })
}

fn outputs(running: &RunningInvention, id: &str) -> Vec<String> {
    running.module_ports.lock().unwrap()[id].outputs.clone()
}

#[test]
fn an_edit_built_before_a_reload_commits_is_built_again_from_the_new_definition() {
    let (mut running, pump, gate) = start_gated();
    let (built, release) = gate.pause();
    let edit = add_voice(running.controller());

    // The edit has built `late` from the v1 definition and not yet
    // committed it; a reload changing the definition commits first.
    built.recv_timeout(WAIT).expect("the edit builds");
    running.reload(document("v2")).expect("reload applies");
    release.send(()).unwrap();
    edit.join().unwrap().expect("the edit commits");

    // The v1 build was refused and dropped; `late` is the v2 rebuild.
    assert_eq!(gate.built(), ["v1", "v2"]);
    assert_eq!(outputs(&running, "late"), ["audio", "aux"]);
    pump.render(1);
}

#[test]
fn a_controller_that_outlives_a_reload_builds_from_the_new_definition() {
    let (mut running, pump, gate) = start_gated();
    let controller = running.controller();
    running.reload(document("v2")).expect("reload applies");

    add_voice(controller)
        .join()
        .unwrap()
        .expect("the edit commits");
    assert_eq!(gate.built(), ["v2"]);
    assert_eq!(outputs(&running, "late"), ["audio", "aux"]);
    pump.render(1);
}

#[test]
fn an_edit_reloads_keep_superseding_is_refused_after_three_builds() {
    let (mut running, pump, gate) = start_gated();
    let pauses: Vec<_> = (0..3).map(|_| gate.pause()).collect();
    let generation = running.live.generation();
    let edit = add_voice(running.controller());

    for ((built, release), tag) in pauses.into_iter().zip(["v2", "v1", "v2"]) {
        built.recv_timeout(WAIT).expect("the edit builds");
        running.reload(document(tag)).expect("reload applies");
        release.send(()).unwrap();
    }
    assert!(matches!(
        edit.join().unwrap(),
        Err(GraphCommandError::TopologyMoved)
    ));

    // Each build was refused; nothing of the edit landed.
    assert_eq!(gate.built(), ["v1", "v2", "v1"]);
    assert_eq!(running.live.generation(), generation);
    assert!(!running.state.lock().unwrap().modules.contains_key("late"));
    pump.render(1);
}
