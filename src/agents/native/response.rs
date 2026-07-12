use super::*;

pub(super) fn parse_json_response(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).or_else(|_| {
        let start = text
            .find('{')
            .ok_or_else(|| "response is not JSON".to_string())?;
        let end = text
            .rfind('}')
            .ok_or_else(|| "response is not JSON".to_string())?;
        serde_json::from_str(&text[start..=end]).map_err(|err| err.to_string())
    })
}

/// Validates the parsed response before any graph writes occur.
///
/// For `json` responses, Fugue expects the standard envelope
/// `{ kind, summary, payload, confidence, warnings }`. Full JSON Schema
/// validation is intentionally not implemented yet; v1 validates the
/// built-in `fugue.step_pattern.v1` preset and preserves user schemas in the
/// request packet for backend-side structured output.
pub(super) fn validate_response(response_config: &Value, response: &Value) -> Result<(), String> {
    let format = response_config
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("text");
    if format == "raw_json" {
        return Ok(());
    }

    for key in ["kind", "summary", "payload", "confidence", "warnings"] {
        if response.get(key).is_none() {
            return Err(format!(
                "agent JSON response missing envelope key '{}'",
                key
            ));
        }
    }

    if response_config.get("schema_ref").and_then(Value::as_str) == Some("fugue.step_pattern.v1") {
        validate_step_pattern(
            response
                .get("payload")
                .and_then(|payload| payload.get("pattern"))
                .ok_or_else(|| "step pattern response missing payload.pattern".to_string())?,
        )?;
    }
    Ok(())
}

pub(super) fn validate_step_pattern(pattern: &Value) -> Result<(), String> {
    let steps = pattern
        .as_array()
        .ok_or_else(|| "payload.pattern must be an array".to_string())?;
    if steps.is_empty() || steps.len() > 64 {
        return Err("payload.pattern length must be 1..=64".to_string());
    }
    for step in steps {
        let Some(object) = step.as_object() else {
            return Err("each pattern step must be an object".to_string());
        };
        match object.get("note") {
            Some(Value::Null) => {}
            Some(Value::Number(number)) if number.as_i64().is_some() => {}
            _ => return Err("step.note must be an integer or null".to_string()),
        }
        if let Some(gate) = object.get("gate").and_then(Value::as_f64) {
            if !(0.0..=1.0).contains(&gate) {
                return Err("step.gate must be between 0 and 1".to_string());
            }
        }
    }
    Ok(())
}

/// Applies validated JSON response fields to configured target controls.
///
/// Writes are explicit and opt-in. The agent never infers graph mutations
/// from model output; each write must name a source JSON path, destination
/// module, control key, and value type.
pub(super) fn apply_response(
    module_id: &str,
    config: &Value,
    controller: &RuntimeController,
    response: &Value,
) -> Result<(), String> {
    let Some(mappings) = config.get("apply").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut writes = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let path = mapping
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| "apply mapping missing from".to_string())?;
        let target = mapping
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| "apply mapping missing to".to_string())?;
        let control = mapping
            .get("control")
            .and_then(Value::as_str)
            .ok_or_else(|| "apply mapping missing control".to_string())?;
        let write_type = mapping
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string");
        let value = get_json_path(response, path)
            .ok_or_else(|| format!("apply path '{}' not found", path))?;
        let control_value = match write_type {
            "number" => {
                let mut number = value
                    .as_f64()
                    .ok_or_else(|| format!("apply path '{}' is not a number", path))?
                    as f32;
                if let Some(min) = mapping.get("min").and_then(Value::as_f64) {
                    number = number.max(min as f32);
                }
                if let Some(max) = mapping.get("max").and_then(Value::as_f64) {
                    number = number.min(max as f32);
                }
                ControlValue::Number(number)
            }
            "bool" => ControlValue::Bool(
                value
                    .as_bool()
                    .ok_or_else(|| format!("apply path '{}' is not a bool", path))?,
            ),
            "json_string" => ControlValue::String(value.to_string()),
            "string" => ControlValue::String(
                value
                    .as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| value.to_string()),
            ),
            other => return Err(format!("unknown apply type '{}'", other)),
        };
        let controls = controller
            .snapshot
            .list_controls(Some(target))
            .map_err(|err| err.to_string())?;
        let has_control = controls
            .first()
            .map(|(_, controls)| controls.iter().any(|meta| meta.key == control))
            .unwrap_or(false);
        if !has_control {
            return Err(format!("unknown apply control '{}:{}'", target, control));
        }
        writes.push((target.to_string(), control.to_string(), control_value));
    }

    for (target, control, control_value) in writes {
        controller
            .snapshot
            .set_control(&target, &control, control_value)
            .map_err(|err| {
                let message = err.to_string();
                set_string(controller, module_id, "last_apply_error", &message);
                message
            })?;
    }
    Ok(())
}
