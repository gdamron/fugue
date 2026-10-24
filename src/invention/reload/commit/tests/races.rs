//! Edits racing a reload never leave the retained document disagreeing
//! with the graph.

use super::*;

#[test]
fn an_edit_landing_while_a_control_only_reload_is_prepared_refuses_it() {
    // Only osc1's frequency changes: the reload's structural change is
    // empty, but it was still planned against a graph that has moved.
    let retuned = BASE.replace(r#""frequency": 440.0"#, r#""frequency": 220.0"#);
    let (mut running, pump) = start(BASE);
    let result = reload_with_interleaved_edit(&mut running, &retuned, 1);
    assert!(
        matches!(result, Err(GraphCommandError::TopologyMoved)),
        "{result:?}"
    );

    // Nothing of the reload landed, and the document agrees with the graph.
    let document = running.document().unwrap();
    assert!(document.modules.iter().all(|module| module.id != "spare"));
    assert!(!running.state.lock().unwrap().modules.contains_key("spare"));
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        ControlValue::Number(440.0)
    );

    // Planned again from the same document, the reload applies.
    running.reload(doc(&retuned)).expect("reload applies");
    assert_eq!(running.document(), Some(doc(&retuned)));
    pump.render(1);
}

type Hook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// A module whose one control runs a hook when written, standing in for an
/// edit (a script's, say) landing right after a reload publishes.
#[derive(Clone, Default)]
struct Hooked(Hook);

struct HookedControls(Hook);

impl crate::ControlSurface for HookedControls {
    fn controls(&self) -> Vec<crate::ControlMeta> {
        vec![crate::ControlMeta::number("level", "Level")]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "level" => Ok(ControlValue::Number(0.0)),
            _ => Err(format!("Unknown control: {key}")),
        }
    }

    fn set_control(&self, _key: &str, _value: ControlValue) -> Result<(), String> {
        let hook = self.0.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }
}

impl crate::ModuleFactory for Hooked {
    fn type_id(&self) -> &'static str {
        "hooked"
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
        // Any port-less module will do; only the surface matters here.
        let module = crate::ModuleRegistry::default()
            .build("code", SAMPLE_RATE, &serde_json::json!({}))?
            .module;
        Ok(crate::ModuleBuildResult {
            module,
            handles: Vec::new(),
            control_surface: Some(Arc::new(HookedControls(self.0.clone()))),
            sink: None,
        })
    }
}

#[test]
fn an_edit_landing_right_after_a_reload_publishes_keeps_its_document_change() {
    let base = BASE.replace(
        r#"{ "id": "dac", "type": "dac" }"#,
        r#"{ "id": "dac", "type": "dac" },
        { "id": "hooked", "type": "hooked", "config": { "level": 0.25 } }"#,
    );
    let hooked = Hooked::default();
    let mut registry = crate::ModuleRegistry::default();
    registry.register(hooked.clone());
    let (runtime, _) = InventionBuilder::with_registry(SAMPLE_RATE, registry.clone())
        .build(doc(&base))
        .unwrap();
    let (mut running, pump) = start_manual(runtime);

    // The reload removes `spare`; writing its control update adds `late`
    // through the live graph, after the reload's publication.
    let live = running.live.clone();
    *hooked.0.lock().unwrap() = Some(Box::new(move || {
        live.add_module(SAMPLE_RATE, "late", "oscillator", &serde_json::json!({}))
            .unwrap();
    }));
    let edited = base
        .replace(
            r#"{ "id": "spare", "type": "oscillator", "config": { "frequency": 3.0 } },"#,
            "",
        )
        .replace(r#""level": 0.25"#, r#""level": 0.75"#);
    let report = running.reload(doc(&edited)).expect("reload applies");
    assert_eq!(report.removed, ["spare"]);
    assert_eq!(report.controls_updated, ["hooked.level"]);
    assert!(hooked.0.lock().unwrap().is_none(), "the hook did not run");

    // The later edit stands in both the graph and the document.
    let document = running.document().unwrap();
    let in_document = |id: &str| document.modules.iter().any(|module| module.id == id);
    assert!(in_document("late"));
    assert!(!in_document("spare"));
    assert!(running.state.lock().unwrap().modules.contains_key("late"));
    pump.render(1);
}

/// Plans and prepares `document` against `running`, lets each authored
/// write land on the audio thread, a block apiece, then commits.
fn reload_under_authored_writes(
    (running, pump): (&mut RunningInvention, &Pump),
    document: &str,
    writes: &[(&str, f32)],
) -> Result<super::super::ReloadReport, GraphCommandError> {
    let validated = running.validate_document(doc(document)).unwrap();
    let (base, authored) = (running.live.generation(), running.live.authored_epoch());
    let plan = running.plan_document(&validated).unwrap();
    let adopt = Some((validated.registry, validated.definitions));
    let prepared = running.prepare_plan(base, plan, Some(validated.document), adopt)?;
    for (module_id, value) in writes {
        let value = ControlValue::Number(*value);
        running.set_control(module_id, "frequency", value).unwrap();
        pump.block();
    }
    running.commit_prepared(prepared.planned_at(authored))
}

fn frequency(running: &RunningInvention, id: &str) -> f64 {
    let document = running.document().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == id).unwrap();
    spec.config["frequency"].as_f64().unwrap()
}

#[test]
fn an_authored_write_landing_while_a_reload_is_prepared_stays_unless_the_reload_writes_it() {
    let retuned = BASE.replace(r#""frequency": 440.0"#, r#""frequency": 220.0"#);
    let (mut running, pump) = start(BASE);
    let writes = [("osc1", 330.0), ("osc2", 660.0)];

    reload_under_authored_writes((&mut running, &pump), &retuned, &writes)
        .expect("the reload commits");
    pump.block();

    // The reload's write follows osc1's; osc2's, which it planned without,
    // stays in the document as it plays.
    assert_eq!(frequency(&running, "osc1"), 220.0);
    assert_eq!(frequency(&running, "osc2"), 660.0);
    for (id, value) in [("osc1", 220.0), ("osc2", 660.0)] {
        let playing = running.get_control(id, "frequency").unwrap();
        assert_eq!(playing, ControlValue::Number(value));
    }
}

#[test]
fn a_reload_more_authored_writes_landed_under_than_are_kept_is_refused() {
    let retuned = BASE.replace(r#""frequency": 440.0"#, r#""frequency": 220.0"#);
    let (mut running, pump) = start(BASE);
    let flood: Vec<_> = (0..65).map(|step| ("osc2", 600.0 + step as f32)).collect();

    let result = reload_under_authored_writes((&mut running, &pump), &retuned, &flood);
    assert!(
        matches!(result, Err(GraphCommandError::TopologyMoved)),
        "{result:?}"
    );
    assert_eq!(frequency(&running, "osc1"), 440.0);

    // Planned again, from the document holding them all.
    running.reload(doc(&retuned)).expect("the reload commits");
    assert_eq!(frequency(&running, "osc1"), 220.0);
    pump.render(1);
}
