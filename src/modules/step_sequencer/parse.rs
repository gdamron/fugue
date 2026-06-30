use super::*;

/// Parses a pattern array from JSON.
pub(crate) fn parse_pattern(
    value: Option<&serde_json::Value>,
) -> Result<Vec<Step>, Box<dyn std::error::Error>> {
    let Some(array) = value.and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    let mut pattern = Vec::with_capacity(array.len());

    for step_value in array {
        let step = parse_step(step_value)?;
        pattern.push(step);
    }

    Ok(pattern)
}

/// Parses a single step from JSON.
pub(crate) fn parse_step(value: &serde_json::Value) -> Result<Step, Box<dyn std::error::Error>> {
    // Handle simple null as rest
    if value.is_null() {
        return Ok(Step::rest());
    }

    // Handle object format
    if let Some(obj) = value.as_object() {
        let held = match obj.get("held") {
            Some(serde_json::Value::Bool(value)) => *value,
            Some(_) => return Err("held must be a boolean".into()),
            None => false,
        };

        if held {
            if obj.keys().any(|key| key != "held") {
                return Err("held steps may only contain {\"held\": true}".into());
            }
            return Ok(Step::held());
        }

        let note = match obj.get("note") {
            Some(serde_json::Value::Null) => None,
            Some(n) => n.as_i64().map(|v| v as i8),
            None => None,
        };

        let gate_length = obj
            .get("gate")
            .and_then(|v| v.as_f64())
            .map(|v| (v as f32).clamp(0.0, 1.0));

        return Ok(Step {
            note,
            gate_length,
            held: false,
            amplitude: None,
        });
    }

    // Handle simple integer as note
    if let Some(n) = value.as_i64() {
        return Ok(Step::note(n as i8));
    }

    Err(format!("Invalid step format: {:?}", value).into())
}
