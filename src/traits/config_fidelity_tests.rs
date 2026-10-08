//! A control value recorded into a module's config rebuilds as written,
//! across the built-in catalog.
//!
//! An authored control write records the value into the module's config
//! under the control's key (`authored_document::write_control`), so a saved
//! document must rebuild to the sound that was written.

use super::*;
use crate::ModuleRegistry;

/// Live telemetry, written by the runtime and never authored.
const TELEMETRY: &[&str] = &[
    "agent.history_json",
    "agent.last_apply_error",
    "agent.last_error",
    "agent.last_response",
    "agent.last_response_json",
    "agent.request_count",
    "agent.reset_count",
    "agent.status",
    "agent.trigger_count",
    "code.last_error",
    "code.status",
];

/// Controls whose value is checked elsewhere: an action rather than a state
/// (a pad's `trigger`, a note, a sequence `advance`), an asset load
/// (`source`), or a schedule that needs a runtime to resolve against (see
/// the scheduler's own tests).
const ELSEWHERE: &[&str] = &[
    "sample_kit.trigger",
    "sample_instrument.note_on",
    "sample_instrument.note_off",
    "cell_sequencer.advance",
    "sample_player.source",
    "control_scheduler.schedule",
];

/// Values to try, in order, until one changes the control.
fn probes(key: &str, meta: &ControlMeta) -> Vec<ControlValue> {
    let text = |values: &[&str]| -> Vec<ControlValue> {
        values
            .iter()
            .map(|v| ControlValue::String(v.to_string()))
            .collect()
    };
    match (&meta.kind, key) {
        (_, "pattern") => text(&[r#"[{"note": 64, "gate": 1.0}, null]"#, r#"[64, null, 67]"#]),
        (_, "sequences_json") => text(&[r#"[[{"note": 64}, null]]"#, r#"[[64, null, 67]]"#]),
        (
            ControlKind::String {
                options: Some(options),
            },
            _,
        ) => text(&options.iter().map(String::as_str).collect::<Vec<_>>()),
        (ControlKind::String { options: None }, _) => text(&["a phrase", "local:other"]),
        (ControlKind::Bool, _) => vec![ControlValue::Bool(true), ControlValue::Bool(false)],
        (ControlKind::Number { .. }, _) => [0.25, 0.5, 3.0, 60.0, 2.0]
            .map(ControlValue::Number)
            .to_vec(),
    }
}

/// The config each type is built from before probing: enough that every
/// control has a value to change to.
fn base_config(type_id: &str) -> serde_json::Value {
    match type_id {
        "cell_sequencer" => serde_json::json!({ "sequences": [[60, null], [62]] }),
        _ => serde_json::json!({}),
    }
}

#[test]
fn every_control_rebuilds_from_the_value_recorded_in_its_config() {
    let registry = ModuleRegistry::default();
    let mut misses = Vec::new();
    let mut checked = 0;
    for type_id in registry.types() {
        if registry.is_sink(type_id) {
            continue;
        }
        let base = base_config(type_id);
        let Ok(built) = registry.build(type_id, 48_000, &base) else {
            continue;
        };
        let Some(surface) = built.control_surface else {
            continue;
        };
        for meta in surface.controls() {
            let name = format!("{type_id}.{}", meta.key);
            if TELEMETRY.contains(&name.as_str()) || ELSEWHERE.contains(&name.as_str()) {
                continue;
            }
            let key = meta.key.as_str();
            let Ok(before) = surface.get_control(key) else {
                continue;
            };
            // The first probe the setter accepts and that changes the value.
            let applied = probes(key, &meta).into_iter().find_map(|value| {
                let value = surface.coerce_value(key, value);
                surface.set_control(key, value).ok()?;
                let applied = surface.get_control(key).ok()?;
                (applied != before).then_some(applied)
            });
            let Some(applied) = applied else {
                // Read-only controls take no write; anything else must.
                if surface.set_control(key, before.clone()).is_ok() {
                    misses.push(format!("{name}: no probe changed it"));
                }
                continue;
            };
            let mut document = crate::Invention::from_json(
                r#"{"version": "1.0.0", "modules": [{"id": "m", "type": "x"}], "connections": []}"#,
            )
            .unwrap();
            document.modules[0].config = base.clone();
            crate::invention::authored_document::write_control(&mut document, "m", key, &applied);
            let config = &document.modules[0].config;
            let rebuilt = registry
                .build(type_id, 48_000, config)
                .map_err(|error| error.to_string())
                .and_then(|built| {
                    built
                        .control_surface
                        .ok_or_else(|| "no controls".to_string())?
                        .get_control(key)
                });
            match rebuilt {
                Ok(value) if value == applied => checked += 1,
                other => misses.push(format!("{name} = {applied:?} rebuilt as {other:?}")),
            }
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
    assert!(checked > 60, "only {checked} controls checked");
}
