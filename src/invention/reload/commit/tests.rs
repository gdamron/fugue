use std::sync::{Arc, Mutex};

use crate::invention::builder::InventionBuilder;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::invention::orchestration::OrchestrationRuntime;
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::{ControlValue, Invention};

mod races;
mod registry;

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

/// BASE with osc1's frequency changed (a control update), osc2 and spare
/// removed, osc3 added, osc1 disconnected from the dac, and osc3 wired to
/// the dac and into osc1's FM input.
const EDITED: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "waveform": "sine", "frequency": 220.0 } },
        { "id": "osc3", "type": "oscillator", "config": { "waveform": "square", "frequency": 330.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc3", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc1", "from_port": "audio", "to": "osc3", "to_port": "frequency_mod" }
    ]
}"#;

fn doc(json: &str) -> Invention {
    Invention::from_json(json).unwrap()
}

fn start(json: &str) -> (RunningInvention, Pump) {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc(json)).unwrap();
    start_manual(runtime)
}

/// Publications made, and publications the audio thread has installed.
fn publications(running: &RunningInvention) -> (u64, u64) {
    let publisher = running.live.publisher().lock().unwrap();
    (publisher.generation(), publisher.applied())
}

#[test]
fn a_multi_mutation_reload_reaches_the_audio_thread_as_one_publication() {
    let (mut running, pump) = start(BASE);
    pump.render(2);
    let (generation, applied) = publications(&running);

    let report = running.reload(doc(EDITED)).expect("reload applies");
    assert_eq!(report.added, ["osc3"]);
    assert_eq!(report.removed, ["osc2", "spare"]);
    assert!(report.swapped.is_empty());
    assert_eq!(report.controls_updated, ["osc1.frequency"]);
    assert_eq!(report.connections_added, 2);
    assert_eq!(report.connections_removed, 1);

    // One publication queued for the whole reload, and nothing installed
    // until the audio thread's next block.
    assert_eq!(publications(&running), (generation + 1, applied));
    pump.render(1);
    assert_eq!(publications(&running), (generation + 1, applied + 1));
    pump.render(4);
    assert_eq!(publications(&running), (generation + 1, applied + 1));

    // The runtime's mirrors and document all reflect the new document.
    let state = running.state.lock().unwrap();
    let ids: Vec<&str> = state.modules.keys().map(String::as_str).collect();
    assert_eq!(ids, ["osc1", "dac", "osc3"]);
    assert_eq!(state.connections.len(), 2);
    drop(state);
    assert_eq!(running.document(), Some(doc(EDITED)));
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        ControlValue::Number(220.0)
    );
}

#[test]
fn an_untouched_module_keeps_its_phase_and_state_across_a_reload() {
    let (mut edited, edited_pump) = start(BASE);
    let (control, control_pump) = start(BASE);
    // Diverge osc2 at runtime in both with a live (performed) tweak, so a
    // rebuild would be audible. An authored write would be reverted by the
    // reload instead.
    for running in [&edited, &control] {
        running
            .snapshot()
            .set_control_with_intent(
                "osc2",
                "frequency",
                ControlValue::Number(123.0),
                crate::ControlWriteIntent::Perform,
            )
            .unwrap();
    }
    assert_eq!(edited_pump.render(7), control_pump.render(7));

    // Remove the unheard module and add another; osc1, osc2 and the dac
    // are untouched.
    let next = BASE.replace(
        r#"{ "id": "spare", "type": "oscillator", "config": { "frequency": 3.0 } }"#,
        r#"{ "id": "lfo", "type": "lfo", "config": { "rate": 2.0 } }"#,
    );
    let report = edited.reload(doc(&next)).expect("reload applies");
    assert_eq!((report.added.len(), report.removed.len()), (1, 1));

    // Sample for sample the same as a runtime that never reloaded: the
    // survivors kept their phase and runtime state through the swap.
    assert_eq!(edited_pump.render(20), control_pump.render(20));
}

#[test]
fn a_failed_preparation_leaves_the_running_invention_unchanged() {
    let (mut running, pump) = start(BASE);
    // Kept alive: dropping a runtime stops its backend.
    let (_twin, twin_pump) = start(BASE);
    assert_eq!(pump.render(3), twin_pump.render(3));

    let observe = |running: &RunningInvention| {
        let state = running.state.lock().unwrap();
        let surfaces: Vec<(String, Vec<String>)> = running
            .list_all_controls()
            .into_iter()
            .map(|(id, controls)| (id, controls.into_iter().map(|meta| meta.key).collect()))
            .collect();
        (
            state.modules.clone(),
            state.connections.clone(),
            state.document.clone(),
            surfaces,
            running.get_control("osc1", "frequency").unwrap(),
            publications(running),
        )
    };
    let before = observe(&running);

    // A plan for EDITED, then made to fail at preparation: on a module
    // build, on a connection to a port that does not exist, and on a control
    // value the control refuses.
    let corruptions: [fn(&mut super::ReloadPlan); 3] = [
        |plan| {
            plan.added[0].module_type = "lfo".into();
            plan.added[0].config = serde_json::json!({ "waveform": "bogus" });
        },
        |plan| plan.added_connections[0].to_port = "no_such_port".into(),
        |plan| plan.control_updates[0].2 = ControlValue::String("loud".into()),
    ];
    for corrupt in corruptions {
        let validated = running.validate_document(doc(EDITED)).unwrap();
        let base = running.live.generation();
        let mut plan = running.plan_document(&validated).unwrap();
        assert!(!plan.control_updates.is_empty() && !plan.removed.is_empty());
        corrupt(&mut plan);
        let adopt = Some((validated.registry, validated.definitions));
        let prepared = running.prepare_plan(base, plan, Some(validated.document), adopt);
        assert!(prepared.is_err());
        assert_eq!(observe(&running), before);
    }

    // Nothing reached the audio thread: it still plays the untouched graph.
    assert_eq!(pump.render(10), twin_pump.render(10));
    assert_eq!(observe(&running), before);

    // And the runtime still reloads normally afterwards.
    running.reload(doc(EDITED)).expect("reload applies");
    assert_eq!(running.document(), Some(doc(EDITED)));
}

/// Plans and prepares `document` against `running`, with an edit removing
/// `spare` landing after `edit_after` steps (0: after planning, 1: after
/// preparing); returns what committing gives.
fn reload_with_interleaved_edit(
    running: &mut RunningInvention,
    document: &str,
    edit_after: usize,
) -> Result<super::ReloadReport, GraphCommandError> {
    let validated = running.validate_document(doc(document)).unwrap();
    let base = running.live.generation();
    let plan = running.plan_document(&validated).unwrap();
    let adopt = Some((validated.registry, validated.definitions));
    let edit = |running: &RunningInvention| running.remove_module("spare").unwrap();
    if edit_after == 0 {
        edit(running);
    }
    let prepared = running.prepare_plan(base, plan, Some(validated.document), adopt)?;
    if edit_after == 1 {
        edit(running);
    }
    running.commit_prepared(prepared)
}

#[test]
fn an_edit_landing_while_a_reload_is_planned_or_prepared_refuses_it() {
    for edit_after in [0, 1] {
        let (mut running, pump) = start(BASE);
        let (twin, twin_pump) = start(BASE);
        twin.remove_module("spare").unwrap();
        let result = reload_with_interleaved_edit(&mut running, EDITED, edit_after);
        assert!(
            matches!(result, Err(GraphCommandError::TopologyMoved)),
            "edit after step {edit_after}: {result:?}"
        );

        // Only the interleaved edit landed: the graph, mirrors and document
        // match a twin that made the same edit alone.
        assert_eq!(pump.render(10), twin_pump.render(10));
        assert_eq!(running.document(), twin.document());
        assert_eq!(running.list_modules(), twin.list_modules());

        // Planned again from the same document, the reload applies.
        running.reload(doc(EDITED)).expect("reload applies");
        assert_eq!(running.document(), Some(doc(EDITED)));
    }
}

/// A module whose one control validates but refuses every write, standing
/// in for a write only making it can reveal as bad (a sample that does not
/// load).
struct Flaky;

struct FlakyControls(f32, String);

impl crate::ControlSurface for FlakyControls {
    fn controls(&self) -> Vec<crate::ControlMeta> {
        vec![crate::ControlMeta::number("level", "Level")]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "level" => Ok(ControlValue::Number(self.0)),
            _ => Err(format!("Unknown control: {key}")),
        }
    }

    fn set_control(&self, _key: &str, _value: ControlValue) -> Result<(), String> {
        Err(self.1.clone())
    }
}

impl crate::ModuleFactory for Flaky {
    fn type_id(&self) -> &'static str {
        "flaky"
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        const {
            &[
                crate::module_config::ConfigKey::float("level"),
                crate::module_config::ConfigKey::text("error"),
            ]
        }
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
        let level = config.get("level").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let error = config.get("error").and_then(|v| v.as_str());
        // Any port-less module will do; only the surface matters here.
        let registry = crate::ModuleRegistry::default();
        let module = registry
            .build("code", 48_000, &serde_json::json!({}))?
            .module;
        Ok(crate::ModuleBuildResult {
            module,
            handles: Vec::new(),
            control_surface: Some(std::sync::Arc::new(FlakyControls(
                level as f32,
                error.unwrap_or("refused when written").to_string(),
            ))),
            sink: None,
        })
    }
}

/// Starts BASE plus a `flaky` module with `config`; returns the document
/// too.
fn start_with_flaky(config: serde_json::Value) -> (RunningInvention, Pump, String) {
    let base = BASE.replace(
        r#"{ "id": "dac", "type": "dac" }"#,
        &format!(r#"{{ "id": "dac", "type": "dac" }}, {{ "id": "flaky", "type": "flaky", "config": {config} }}"#),
    );
    let mut registry = crate::ModuleRegistry::default();
    registry.register(Flaky);
    let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry)
        .build(doc(&base))
        .unwrap();
    let (running, pump) = start_manual(runtime);
    (running, pump, base)
}

#[test]
fn a_control_write_that_fails_when_made_is_reported_and_not_retained() {
    let (mut running, pump, base) = start_with_flaky(serde_json::json!({ "level": 0.25 }));
    let edited = base
        .replace(r#""level":0.25"#, r#""level":0.75"#)
        .replace(r#""frequency": 440.0"#, r#""frequency": 220.0"#);
    let report = running.reload(doc(&edited)).expect("reload applies");
    assert_eq!(report.controls_updated, ["osc1.frequency"]);
    assert_eq!(report.controls_failed.len(), 1);
    let failure = &report.controls_failed[0];
    assert_eq!(
        (failure.module_id.as_str(), failure.key.as_str()),
        ("flaky", "level")
    );
    assert_eq!(failure.error, "refused when written");

    // The document records what the module plays, not what was asked.
    let document = running.document().unwrap();
    let flaky = document.modules.iter().find(|m| m.id == "flaky").unwrap();
    assert_eq!(flaky.config["level"], serde_json::json!(0.25));
    // So does the stored config, so reloading the same document tries the
    // write again rather than seeing no change.
    let stored = running.state.lock().unwrap().modules["flaky"]
        .config
        .clone();
    assert_eq!(stored["level"], serde_json::json!(0.25));
    let report = running.reload(doc(&edited)).expect("reload applies");
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert_eq!(report.controls_failed.len(), 1, "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    pump.render(1);
}

#[test]
fn a_reported_control_error_is_cut_short_on_a_character_boundary() {
    // One ASCII byte shifts every two-byte "é" so the cap lands mid-character
    // and the cut has to back off by one.
    let long = format!("a{}", "é".repeat(crate::rpc::MODULE_ERROR_BYTES));
    let config = serde_json::json!({ "level": 0.25, "error": long });
    let (mut running, _pump, base) = start_with_flaky(config);
    let edited = base.replace(r#""level":0.25"#, r#""level":0.75"#);
    let report = running.reload(doc(&edited)).expect("reload applies");

    let error = &report.controls_failed[0].error;
    assert!(!long.is_char_boundary(crate::rpc::MODULE_ERROR_BYTES));
    assert_eq!(error.len(), crate::rpc::MODULE_ERROR_BYTES - 1);
    assert!(long.is_char_boundary(error.len()));
    assert!(long.starts_with(error.as_str()));
}
