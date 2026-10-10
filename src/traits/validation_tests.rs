//! `validate_control` agrees with `set_control` across the built-in catalog.

use std::sync::{Arc, Mutex};

use super::*;
use crate::ModuleRegistry;

/// Values that probe kinds, options, aliases and JSON parsing.
fn probes(meta: &ControlMeta) -> Vec<ControlValue> {
    let mut values = vec![meta.default.clone()];
    values.extend([0.0, 0.5, 60.0, -3.0, 1e3].map(ControlValue::Number));
    values.extend([true, false].map(ControlValue::Bool));
    values.extend(
        [
            "", "0.5", "60", "sine", "Saw", "tri", "lpf", "bandpass", "loop", "one_shot",
            "on_beat", "before", "bogus", "[]", "[{}]", "{", "true",
        ]
        .map(|text| ControlValue::String(text.to_string())),
    );
    values
}

/// Every listed key of every built-in surface: a write the validator
/// accepts is one the setter accepts, and the reverse. Only asset loads are
/// left to the write, and non-finite numbers are refused before any setter
/// sees them.
#[test]
fn validation_refuses_exactly_what_setters_refuse() {
    let registry = ModuleRegistry::default();
    let no_surfaces = ControlSurfaceMap::new();
    let mut checked = 0;
    for type_id in registry.types() {
        if registry.is_sink(type_id) {
            continue;
        }
        let Ok(built) = registry.build(type_id, 48_000, &serde_json::json!({})) else {
            continue;
        };
        // A scheduler's setter needs the runtime it is attached to; every
        // write a runtime makes reaches an attached one.
        let directory = Arc::new(Mutex::new(ControlSurfaceMap::new()));
        if type_id == crate::modules::control_scheduler::CONTROL_SCHEDULER_TYPE_ID {
            crate::modules::control_scheduler::attach_from_handle(
                "sched",
                built.handles.first().map(|(_, handle)| handle),
                &directory,
            )
            .unwrap();
        }
        let Some(surface) = built.control_surface else {
            continue;
        };
        let mut metas = surface.controls();
        metas.push(ControlMeta::number("no_such_control", ""));
        for meta in metas {
            let key = meta.key.as_str();
            if key == "asset" || key.starts_with("asset.") {
                continue;
            }
            for value in probes(&meta) {
                let value = surface.coerce_value(key, value);
                let valid = surface.validate_control(key, &value, &no_surfaces);
                let set = surface.set_control(key, value.clone());
                assert_eq!(
                    valid.is_ok(),
                    set.is_ok(),
                    "{type_id}.{key} = {value:?}: validate {valid:?}, set {set:?}"
                );
                checked += 1;
            }
            if matches!(meta.kind, ControlKind::Number { .. }) {
                let nan = ControlValue::Number(f32::NAN);
                assert!(surface.validate_control(key, &nan, &no_surfaces).is_err());
            }
        }
    }
    assert!(checked > 1000, "only {checked} writes checked");
}

#[test]
fn a_schedule_validates_against_the_directory_the_write_lands_in() {
    let registry = ModuleRegistry::default();
    let built = registry
        .build("control_scheduler", 48_000, &serde_json::json!({}))
        .unwrap();
    let directory = Arc::new(Mutex::new(ControlSurfaceMap::new()));
    crate::modules::control_scheduler::attach_from_handle(
        "sched",
        built.handles.first().map(|(_, handle)| handle),
        &directory,
    )
    .unwrap();
    let surface = built.control_surface.unwrap();
    let schedule = ControlValue::String(
        r#"[{ "at_step": 0, "module": "osc", "control": "frequency", "value": 220.0 }]"#
            .to_string(),
    );

    // The target exists only in the candidate directory.
    let mut candidate = ControlSurfaceMap::new();
    let osc = registry.build("oscillator", 48_000, &serde_json::json!({}));
    candidate.insert("osc".to_string(), osc.unwrap().control_surface.unwrap());
    assert!(surface
        .validate_control("schedule", &schedule, &candidate)
        .is_ok());
    assert!(surface
        .validate_control("schedule", &schedule, &ControlSurfaceMap::new())
        .is_err());
    assert!(surface
        .validate_control("step", &ControlValue::Number(1.0), &candidate)
        .is_err());
}

/// A development's exposed `schedule` names a module inside the
/// development; the outer document has no such module.
fn development_with_inner_scheduler() -> crate::Invention {
    use crate::{DevelopmentControl, DevelopmentSpec, Invention, ModuleSpec};
    let module = |id: &str, module_type: &str| ModuleSpec {
        id: id.to_string(),
        module_type: module_type.to_string(),
        config: serde_json::json!({}),
    };
    let blank = |modules| Invention {
        version: "1.0.0".to_string(),
        title: None,
        description: None,
        developments: vec![],
        assets: Default::default(),
        modules,
        connections: vec![],
        inputs: vec![],
        outputs: vec![],
        controls: vec![],
        source_path: None,
    };
    let mut pattern = blank(vec![
        module("sched", "control_scheduler"),
        module("o", "oscillator"),
    ]);
    pattern.controls.push(DevelopmentControl {
        name: "schedule".to_string(),
        module: "sched".to_string(),
        control: "schedule".to_string(),
    });
    let mut root = blank(vec![module("lead", "pattern")]);
    root.developments.push(DevelopmentSpec {
        reference: None,
        name: "pattern".to_string(),
        path: None,
        definition: Some(Box::new(pattern)),
    });
    root
}

#[test]
fn a_development_validates_inner_writes_against_its_own_directory() {
    let (runtime, _) = crate::InventionBuilder::new(48_000)
        .build(development_with_inner_scheduler())
        .unwrap();
    let surface = runtime
        .control_surfaces
        .lock()
        .unwrap()
        .get("lead")
        .cloned()
        .unwrap();
    let schedule = |module: &str| {
        ControlValue::String(format!(
            r#"[{{ "at_step": 0, "module": "{module}", "control": "frequency", "value": 220.0 }}]"#
        ))
    };

    // The outer directory plays no part: `o` resolves inside the
    // development, and `lead` (an outer module) does not.
    let mut outer = ControlSurfaceMap::new();
    outer.insert("lead".to_string(), surface.clone());
    assert!(surface
        .validate_control("schedule", &schedule("o"), &outer)
        .is_ok());
    assert!(surface
        .validate_control("schedule", &schedule("lead"), &outer)
        .is_err());

    // The setter resolves against the same directory, which the development
    // keeps after build: it takes what validation accepts and refuses what
    // validation refuses.
    surface
        .set_control("schedule", schedule("o"))
        .expect("the inner scheduler resolves `o`");
    assert!(surface.set_control("schedule", schedule("lead")).is_err());
    // The refused write left the accepted one in place.
    let kept = surface.get_control("schedule").unwrap();
    assert!(
        kept.as_string().unwrap().contains(r#""module":"o""#),
        "{kept:?}"
    );
}

#[test]
fn a_scheduler_not_yet_attached_validates_its_targets() {
    // An edit batch checks a scheduler it adds before attaching it.
    let registry = ModuleRegistry::default();
    let built = registry
        .build("control_scheduler", 48_000, &serde_json::json!({}))
        .unwrap();
    let surface = built.control_surface.unwrap();
    let schedule = ControlValue::String(
        r#"[{ "at_step": 0, "module": "osc", "control": "frequency", "value": 220.0 }]"#
            .to_string(),
    );
    let mut candidate = ControlSurfaceMap::new();
    let osc = registry.build("oscillator", 48_000, &serde_json::json!({}));
    candidate.insert("osc".to_string(), osc.unwrap().control_surface.unwrap());

    assert!(surface
        .validate_control("schedule", &schedule, &candidate)
        .is_ok());
    assert!(surface
        .validate_control("schedule", &schedule, &ControlSurfaceMap::new())
        .is_err());
    assert!(surface
        .validate_control("schedule", &ControlValue::String("{".into()), &candidate)
        .is_err());
}
