use super::*;

#[derive(Clone, Debug)]
pub(super) struct HistoryLimits {
    pub(super) max_turns: usize,
    pub(super) max_chars: usize,
}


/// Builds the structured packet sent to local/API backends.
///
/// The packet separates task text, explicit context bindings, optional
/// graph summary, bounded history, and response instructions so adapters do
/// not need to scrape prose to recover structured data.
pub(super) fn build_request_packet(
    module_id: &str,
    config: &Value,
    controller: &RuntimeController,
    history: &[Value],
    limits: &HistoryLimits,
) -> Result<Value, String> {
    let prompt = get_string(controller, module_id, "prompt")
        .or_else(|| string_config(config, "prompt"))
        .unwrap_or_default();
    let system = get_string(controller, module_id, "system")
        .or_else(|| string_config(config, "system"))
        .unwrap_or_default();

    let mut packet = json!({
        "module_id": module_id,
        "system": system,
        "task": prompt,
        "context": build_context(config, controller)?,
        "history": history_for_prompt(history, limits),
        "response": config.get("response").cloned().unwrap_or(Value::Null)
    });

    if config
        .get("include_graph_summary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        packet["graph"] = graph_summary(controller);
    }

    Ok(packet)
}

/// Resolves explicit context bindings against the runtime.
///
/// Supported sources are original module config, a single control, or a set
/// of controls matched by exact key or simple suffix wildcard such as
/// `degree.*`.
pub(super) fn build_context(config: &Value, controller: &RuntimeController) -> Result<Value, String> {
    let mut map = serde_json::Map::new();
    let Some(bindings) = config.get("context").and_then(Value::as_array) else {
        return Ok(Value::Object(map));
    };

    let modules = controller.snapshot.list_modules();
    for binding in bindings {
        let name = binding
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "agent context binding missing name".to_string())?;
        let module_id = binding
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("agent context binding '{}' missing from", name))?;
        let source = binding
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("agent context binding '{}' missing source", name))?;

        let value = match source {
            "config" => {
                let module = modules
                    .iter()
                    .find(|module| module.id == module_id)
                    .ok_or_else(|| format!("unknown context module '{}'", module_id))?;
                if let Some(path) = binding.get("path").and_then(Value::as_str) {
                    get_path(&module.config, path).cloned().ok_or_else(|| {
                        format!("config path '{}' not found on '{}'", path, module_id)
                    })?
                } else {
                    module.config.clone()
                }
            }
            "control" => {
                let key = binding
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("control binding '{}' missing key", name))?;
                control_value_to_json(
                    controller
                        .snapshot
                        .get_control(module_id, key)
                        .map_err(|e| e.to_string())?,
                )
            }
            "controls" => {
                let keys = binding
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("controls binding '{}' missing keys", name))?;
                let controls = controller
                    .snapshot
                    .list_controls(Some(module_id))
                    .map_err(|e| e.to_string())?;
                let available = controls
                    .first()
                    .map(|(_, controls)| controls.clone())
                    .unwrap_or_default();
                let mut controls_map = serde_json::Map::new();
                for wanted in keys.iter().filter_map(Value::as_str) {
                    for control in &available {
                        if key_matches(wanted, &control.key) {
                            let value = controller
                                .snapshot
                                .get_control(module_id, &control.key)
                                .map_err(|e| e.to_string())?;
                            controls_map
                                .insert(control.key.clone(), control_value_to_json(value));
                        }
                    }
                }
                Value::Object(controls_map)
            }
            other => return Err(format!("unknown agent context source '{}'", other)),
        };
        map.insert(name.to_string(), value);
    }

    Ok(Value::Object(map))
}

pub(super) fn graph_summary(controller: &RuntimeController) -> Value {
    json!({
        "status": controller.snapshot.status(),
        "modules": controller.snapshot.list_modules().into_iter().map(|module| {
            json!({ "id": module.id, "type": module.module_type })
        }).collect::<Vec<_>>(),
        "connections": controller.snapshot.list_connections().into_iter().map(|conn| {
            json!({
                "from": format!("{}:{}", conn.from, conn.from_port),
                "to": format!("{}:{}", conn.to, conn.to_port)
            })
        }).collect::<Vec<_>>()
    })
}
