use super::*;
use crate::module_config::{finite_f32, whole_number, ConfigReader, NumberRefusal};
use std::fmt;

/// Why a step was refused: a number at a path within the step (`note`,
/// `grace[1]`, or empty for a bare number step), or the step's shape.
#[derive(Debug)]
pub(crate) enum StepError {
    Number(String, NumberRefusal),
    Shape(String),
}

impl StepError {
    /// The error as a refusal of the config of `reader`'s module type.
    pub(crate) fn refused_by(self, reader: &ConfigReader) -> Box<dyn std::error::Error> {
        match self {
            Self::Number(path, refusal) => reader.refuse(&path, refusal).into(),
            Self::Shape(message) => message.into(),
        }
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(path, refusal) if path.is_empty() => write!(f, "step {refusal}"),
            Self::Number(path, refusal) => write!(f, "'{path}' {refusal}"),
            Self::Shape(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StepError {}

impl From<&str> for StepError {
    fn from(message: &str) -> Self {
        Self::Shape(message.to_string())
    }
}

impl From<String> for StepError {
    fn from(message: String) -> Self {
        Self::Shape(message)
    }
}

/// Parses a pattern array from JSON; a refused number names its path from
/// `name` (`pattern[3].note`).
pub(crate) fn parse_pattern(
    value: Option<&serde_json::Value>,
    name: &str,
) -> Result<Vec<Step>, StepError> {
    let Some(array) = value.and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };

    let mut pattern = Vec::with_capacity(array.len());

    for (index, step_value) in array.iter().enumerate() {
        let step = parse_step(step_value).map_err(|error| match error {
            StepError::Number(path, refusal) => {
                let dot = if path.is_empty() { "" } else { "." };
                StepError::Number(format!("{name}[{index}]{dot}{path}"), refusal)
            }
            shape => shape,
        })?;
        pattern.push(step);
    }

    Ok(pattern)
}

/// A step's optional number field: absent or `null` is `None`.
fn number_field<T>(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    read: impl Fn(&serde_json::Value) -> Result<T, NumberRefusal>,
) -> Result<Option<T>, StepError> {
    obj.get(key)
        .filter(|value| !value.is_null())
        .map(|value| read(value).map_err(|refusal| StepError::Number(key.to_string(), refusal)))
        .transpose()
}

/// Parses a single step from JSON.
pub(crate) fn parse_step(value: &serde_json::Value) -> Result<Step, StepError> {
    // Handle simple null as rest
    if value.is_null() {
        return Ok(Step::rest());
    }

    // Handle object format
    if let Some(obj) = value.as_object() {
        if let Some(key) = obj.keys().find(|key| !STEP_KEYS.contains(&key.as_str())) {
            return Err(format!(
                "unknown step field '{}' (expected one of: {})",
                key,
                STEP_KEYS.join(", ")
            )
            .into());
        }
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

        let note = number_field(obj, "note", whole_number::<i8>)?;
        let gate_length = number_field(obj, "gate_length", finite_f32)?.map(|v| v.clamp(0.0, 1.0));
        let velocity = number_field(obj, "velocity", finite_f32)?.map(|v| v.clamp(0.0, 1.0));

        let grace = parse_grace(obj.get("grace"), note)?;

        return Ok(Step {
            note,
            gate_length,
            held: false,
            velocity,
            grace,
        });
    }

    // Handle a bare number as a note
    if value.is_number() {
        let note = whole_number::<i8>(value).map_err(|r| StepError::Number(String::new(), r))?;
        return Ok(Step::note(note));
    }

    Err(format!("Invalid step format: {:?}", value).into())
}

/// Parses the optional `grace` array on a note step. Absent, null, and empty
/// arrays all mean "no grace notes"; a non-empty chain requires a principal
/// note to resolve into.
fn parse_grace(
    value: Option<&serde_json::Value>,
    note: Option<i8>,
) -> Result<GraceChain, StepError> {
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
        offsets[index] = whole_number::<i8>(item)
            .map_err(|refusal| StepError::Number(format!("grace[{index}]"), refusal))?;
    }
    Ok(GraceChain::from_slice(&offsets[..items.len()])?)
}
