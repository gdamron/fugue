//! A development instance's config, written to its exposed controls once
//! it is built.

use crate::invention::load_warnings;
use crate::ControlSurface;

/// Applies a development instance's config as initial values for its exposed
/// controls. A development has no other config surface, so every key must
/// name an exposed control and every value must be a scalar; this is what
/// lets a document whose control changes were written into a development
/// instance's config rebuild with the same values on a cold load. A key
/// naming an event (one an alias fires) holds no state to restore: it is
/// read and ignored, as a plain module's build ignores one, and a reload
/// ignores it too (see `plan_reload`). A value its control refuses, as a
/// live write to it would be refused, is set aside so the rest still
/// start: returned as a load warning each.
pub(super) fn apply_development_config(
    name: &str,
    config: &serde_json::Value,
    surface: &(dyn ControlSurface + Send + Sync),
) -> Result<Vec<load_warnings::Warning>, Box<dyn std::error::Error>> {
    let map = match config {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Object(map) => map,
        _ => return Err(format!("Development '{}' config must be an object", name).into()),
    };
    let mut pending = Vec::with_capacity(map.len());
    for (key, value) in map {
        if surface
            .declaration(key)
            .is_some_and(|found| found.decl.event)
        {
            continue;
        }
        let value = crate::traits::exact_number::json_scalar(value).ok_or_else(|| {
            format!(
                "Development '{}' config key '{}' must be a number, bool, or string",
                name, key
            )
        })?;
        // Number text an f32 misreads stays text only for a whole-number
        // control; any other number control is handed a number.
        pending.push((key, surface.coerce_value(key, value)));
    }
    // A payload (a melody's whole scale) applies before the scalars, which
    // may write within it.
    pending.sort_by_key(|(key, _)| {
        !surface
            .declaration(key)
            .is_some_and(|found| found.decl.kind == crate::control_request::DeclKind::Payload)
    });
    // Keys are applied in passes, a failed one again after the rest, since
    // one control can size what another reaches (a count and a degree
    // behind it). A key still failing whose control is listed but cannot be
    // read is one a count hides now: it was written before the count shrank,
    // and is left out. A value any other control refuses is set aside, and
    // a key naming no control fails the build.
    loop {
        let before = pending.len();
        let mut failed = Vec::new();
        for (key, value) in pending {
            if let Err(error) = surface.set_control(key, value.clone()) {
                failed.push((key, value, error));
            }
        }
        if failed.is_empty() {
            return Ok(Vec::new());
        }
        if failed.len() == before {
            let listed = surface.controls();
            let mut refused = Vec::new();
            for (key, _, error) in failed {
                let is_listed = listed.iter().any(|meta| &meta.key == key);
                if is_listed && surface.get_control(key).is_err() {
                    continue;
                }
                if !is_listed && surface.declaration(key).is_none() {
                    return Err(format!("Development '{}' config: {}", name, error).into());
                }
                refused.push(load_warnings::Warning {
                    modules: Vec::new(),
                    refused: Some(key.clone()),
                    message: format!(
                        "development '{name}' control '{key}' refused its starting value {} \
                         ({error}), so it starts where its definition starts it",
                        map[key.as_str()]
                    ),
                });
            }
            return Ok(refused);
        }
        pending = failed
            .into_iter()
            .map(|(key, value, _)| (key, value))
            .collect();
    }
}
