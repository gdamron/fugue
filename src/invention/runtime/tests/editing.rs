//! Live edits on a running invention, with the test thread rendering its
//! blocks in place of the audio thread.

use super::*;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::invention::publish::tests::probe::{DropProbeFactory, DROP_PROBE};
use crate::ModuleRegistry;

const BASE: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "waveform": "sine", "frequency": 550.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn start(registry: ModuleRegistry) -> (RunningInvention, Pump) {
    let invention = Invention::from_json(BASE).unwrap();
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(invention)
        .unwrap();
    start_manual(runtime)
}

#[test]
fn live_edits_keep_untouched_modules_phase() {
    let (edited, edited_audio) = start(ModuleRegistry::default());
    let (_untouched, untouched_audio) = start(ModuleRegistry::default());
    assert_eq!(edited_audio.render(5), untouched_audio.render(5));

    // An unconnected module comes and goes, once through the runtime and
    // once through a controller (the path scripts and agents use).
    edited
        .add_module(
            "lfo",
            "oscillator",
            &serde_json::json!({ "frequency": 2.0 }),
        )
        .unwrap();
    assert_eq!(edited_audio.render(10), untouched_audio.render(10));
    edited.controller().remove_module("lfo").unwrap();
    assert_eq!(edited_audio.render(10), untouched_audio.render(10));
}

#[test]
fn a_failed_live_edit_changes_nothing() {
    let (running, audio) = start(ModuleRegistry::default());
    let (_untouched, untouched_audio) = start(ModuleRegistry::default());
    assert_eq!(audio.render(2), untouched_audio.render(2));
    let snapshot = |running: &RunningInvention| {
        let state = running.state.lock().unwrap();
        let surfaces: Vec<String> = running
            .control_surfaces
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let ports: Vec<String> = running
            .module_ports
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        (
            state.modules.clone(),
            state.connections.clone(),
            state.document.clone(),
            surfaces,
            ports,
        )
    };
    let before = snapshot(&running);
    let controller = running.controller();

    assert!(running
        .add_module("x", "no_such_type", &serde_json::json!({}))
        .is_err());
    assert!(controller
        .add_module("x", "lfo", &serde_json::json!({ "waveform": "bogus" }))
        .is_err());
    let unresolved = serde_json::json!({
        "schedule": [{ "at_step": 0, "module": "missing", "control": "frequency", "value": 1.0 }]
    });
    assert!(running
        .add_module("sched", "control_scheduler", &unresolved)
        .is_err());
    assert!(running.connect("osc1", "nope", "dac", "audio").is_err());
    assert!(controller
        .connect("ghost", "audio", "dac", "audio")
        .is_err());
    assert!(running
        .swap_module("ghost", "oscillator", &serde_json::json!({}), true)
        .is_err());

    assert_eq!(snapshot(&running), before);
    assert_eq!(audio.render(10), untouched_audio.render(10));
}

#[test]
fn a_sink_removed_live_is_dropped_promptly_off_the_audio_thread() {
    let probes = DropProbeFactory::default();
    let mut registry = ModuleRegistry::default();
    registry.register(probes.clone());
    let (running, audio) = start(registry);
    running
        .add_module("probe", DROP_PROBE, &serde_json::json!({}))
        .unwrap();
    running.connect("osc1", "audio", "probe", "audio").unwrap();
    audio.render(2);

    // The test thread renders, so it stands in for the audio thread. Once
    // the removal installs, no further edit runs: only the runtime's
    // reclaimer can free the probe.
    running.remove_module("probe").unwrap();
    audio.render(1);
    let drops = probes.wait_for_drops(1, Duration::from_secs(2));
    assert_eq!(drops.len(), 1, "the removed sink was not dropped");
    assert_ne!(drops[0], thread::current().id());
}
