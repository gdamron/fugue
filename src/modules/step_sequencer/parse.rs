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

        let amplitude = obj
            .get("amplitude")
            .and_then(|v| v.as_f64())
            .map(|v| (v as f32).clamp(0.0, 1.0));

        let grace = parse_grace(obj.get("grace"), note)?;

        return Ok(Step {
            note,
            gate_length,
            held: false,
            amplitude,
            grace,
        });
    }

    // Handle simple integer as note
    if let Some(n) = value.as_i64() {
        return Ok(Step::note(n as i8));
    }

    Err(format!("Invalid step format: {:?}", value).into())
}

/// Parses the optional `grace` array on a note step. Absent, null, and empty
/// arrays all mean "no grace notes"; a non-empty chain requires a principal
/// note to resolve into.
fn parse_grace(
    value: Option<&serde_json::Value>,
    note: Option<i8>,
) -> Result<GraceChain, Box<dyn std::error::Error>> {
    let items = match value {
        None | Some(serde_json::Value::Null) => return Ok(GraceChain::default()),
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => return Err("step.grace must be an array of integer offsets".into()),
    };

    if items.is_empty() {
        return Ok(GraceChain::default());
    }
    if note.is_none() {
        return Err("step.grace requires a principal note".into());
    }
    if items.len() > MAX_GRACE_NOTES {
        return Err(format!(
            "step.grace holds at most {} offsets (got {})",
            MAX_GRACE_NOTES,
            items.len()
        )
        .into());
    }

    let mut offsets = [0i8; MAX_GRACE_NOTES];
    for (index, item) in items.iter().enumerate() {
        let offset = item.as_i64().ok_or("step.grace entries must be integers")?;
        if !(i8::MIN as i64..=i8::MAX as i64).contains(&offset) {
            return Err(format!(
                "step.grace offset {} out of range (must fit in -128..=127)",
                offset
            )
            .into());
        }
        offsets[index] = offset as i8;
    }
    Ok(GraceChain::from_slice(&offsets[..items.len()])?)
}
