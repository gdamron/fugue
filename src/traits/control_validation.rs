//! Checking a control write against a surface's declared controls, without
//! making it.

use indexmap::IndexMap;
use std::sync::Arc;

use super::{ControlKind, ControlMeta, ControlSurface, ControlValue};

/// Control surfaces keyed by module id: a runtime's directory, or the
/// directory a pending graph change will leave.
pub type ControlSurfaceMap = IndexMap<String, Arc<dyn ControlSurface + Send + Sync>>;

/// Checks `value` against the kind `key` declares in `controls`: an unlisted
/// key is unknown, and the value must be of the declared kind, with numbers
/// finite. String options are left to the surface, whose setter may accept
/// aliases.
pub(crate) fn check_listed_control(
    controls: &[ControlMeta],
    key: &str,
    value: &ControlValue,
) -> Result<(), String> {
    let meta = controls
        .iter()
        .find(|meta| meta.key == key)
        .ok_or_else(|| format!("Unknown control: {}", key))?;
    match (&meta.kind, value) {
        (ControlKind::Number { .. }, ControlValue::Number(number)) => check_finite(key, *number),
        (ControlKind::Number { .. }, _) => value.as_number().map(drop),
        (ControlKind::Bool, _) => value.as_bool().map(drop),
        (ControlKind::String { .. }, _) => value.as_string().map(drop),
    }
}

/// Refuses a number that is not finite.
pub(crate) fn check_finite(key: &str, number: f32) -> Result<(), String> {
    if number.is_finite() {
        Ok(())
    } else {
        Err(format!(
            "Control '{}' needs a finite number, not {}",
            key, number
        ))
    }
}

/// The refusal for a control that can be read but not written.
pub(crate) fn read_only(key: &str) -> Result<(), String> {
    Err(format!("Control '{}' is read-only", key))
}
