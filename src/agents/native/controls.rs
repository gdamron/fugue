use super::*;

pub(super) fn get_number(controller: &RuntimeController, module_id: &str, key: &str) -> f32 {
    match controller.snapshot.get_control(module_id, key) {
        Ok(ControlValue::Number(value)) => value,
        _ => 0.0,
    }
}

pub(super) fn get_bool(controller: &RuntimeController, module_id: &str, key: &str) -> bool {
    match controller.snapshot.get_control(module_id, key) {
        Ok(ControlValue::Bool(value)) => value,
        _ => false,
    }
}

pub(super) fn get_string(
    controller: &RuntimeController,
    module_id: &str,
    key: &str,
) -> Option<String> {
    match controller.snapshot.get_control(module_id, key) {
        Ok(ControlValue::String(value)) => Some(value),
        _ => None,
    }
}

/// Writes an agent telemetry control (`status`, `last_error`, history).
/// Transient: live activity is not authored configuration, so it must not
/// land in the retained document.
pub(super) fn set_string(controller: &RuntimeController, module_id: &str, key: &str, value: &str) {
    let _ = controller.snapshot.set_control_transient(
        module_id,
        key,
        ControlValue::String(value.to_string()),
    );
}

pub(super) fn control_value_to_json(value: ControlValue) -> Value {
    match value {
        ControlValue::Number(value) => json!(value),
        ControlValue::Bool(value) => json!(value),
        ControlValue::String(value) => json!(value),
    }
}

pub(super) fn string_config(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub(super) fn history_limits(config: &Value) -> HistoryLimits {
    let history = config.get("history").unwrap_or(&Value::Null);
    HistoryLimits {
        max_turns: history
            .get("max_turns")
            .and_then(Value::as_u64)
            .unwrap_or(6) as usize,
        max_chars: history
            .get("max_chars")
            .and_then(Value::as_u64)
            .unwrap_or(12_000) as usize,
    }
}

pub(super) fn history_for_prompt(history: &[Value], limits: &HistoryLimits) -> Value {
    let mut items: Vec<Value> = history
        .iter()
        .rev()
        .take(limits.max_turns)
        .cloned()
        .collect();
    items.reverse();
    Value::Array(items)
}

pub(super) fn trim_history(history: &mut Vec<Value>, limits: &HistoryLimits) {
    while history.len() > limits.max_turns {
        history.remove(0);
    }
    while Value::Array(history.clone()).to_string().len() > limits.max_chars && !history.is_empty()
    {
        history.remove(0);
    }
}

pub(super) fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.trim_start_matches("$.").split('.') {
        if part.is_empty() {
            continue;
        }
        current = current.get(part)?;
    }
    Some(current)
}

pub(super) fn get_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    get_path(value, path)
}

pub(super) fn key_matches(pattern: &str, key: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        key.starts_with(prefix)
    } else {
        key == pattern
    }
}

pub(super) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}
