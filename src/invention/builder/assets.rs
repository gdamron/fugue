//! Resolving a document's assets: `$asset` references in module configs and
//! the audio files modules name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::invention::format::Invention;
use crate::invention::reload::LoadedDevelopments;
use crate::registry::ModuleRegistry;

/// Resolves `$asset` references in module configs against the document's
/// declared assets, then resolves module `asset` audio references (package
/// cache → relative to the invention file → absolute; see
/// `crate::invention::audio_assets`). Configs are compared post-resolution at
/// runtime, so reload resolves the new document the same way before diffing.
///
/// Returns the document with the developments it declares, `loaded` when
/// given, otherwise loaded here: a development instance's config holds the
/// controls its development exposes, which name audio only as declared:
/// by `loaded`, then by `registry`, which holds the developments a nested
/// build inherits from where its development is used.
pub(crate) fn resolve_invention_assets(
    mut invention: Invention,
    loaded: Option<Arc<LoadedDevelopments>>,
    registry: Option<&ModuleRegistry>,
) -> Result<(Invention, Arc<LoadedDevelopments>), Box<dyn std::error::Error>> {
    resolve_json_assets(&mut invention)?;
    let loaded = match loaded {
        Some(loaded) => loaded,
        None => Arc::new(LoadedDevelopments::load(&invention)?),
    };
    // After `$asset` expansion, so an audio ref may itself come from a
    // shared JSON asset.
    let declarations = crate::invention::audio_assets::Declarations::of(&loaded, registry);
    crate::invention::audio_assets::resolve_audio_assets(&mut invention, &declarations)?;
    Ok((invention, loaded))
}

/// The `$asset` half of [`resolve_invention_assets`]: loads declared JSON
/// assets and expands `$asset` references in module configs.
pub(crate) fn resolve_json_assets(
    invention: &mut Invention,
) -> Result<(), Box<dyn std::error::Error>> {
    if invention.assets.is_empty() {
        return Ok(());
    }

    let mut assets = HashMap::new();
    for (name, asset) in &invention.assets {
        let resolved = resolve_asset_path(invention.source_path.as_deref(), &asset.path)?;
        let contents = read_asset_text(name, &resolved)?;
        #[cfg(not(target_arch = "wasm32"))]
        crate::pkg::lock::check_bytes(&resolved, contents.as_bytes())?;
        let is_json = resolved
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        let value = if is_json {
            serde_json::from_str(&contents).map_err(|err| {
                format!(
                    "Failed to parse asset '{}' from '{}': {}",
                    name,
                    resolved.display(),
                    err
                )
            })?
        } else {
            serde_json::Value::String(contents)
        };
        // Assets that declare themselves as scores opt into load-time
        // validation against `fugue.score.v1`. Other JSON assets are left
        // as-is, since assets are a general-purpose mechanism.
        if declares_score_schema(&value) {
            crate::invention::score::validate_score(&value).map_err(|err| {
                format!(
                    "Invalid score asset '{}' from '{}': {}",
                    name,
                    resolved.display(),
                    err
                )
            })?;
        }
        assets.insert(name.clone(), value);
    }

    for module in &mut invention.modules {
        module.config = resolve_asset_refs(&module.config, &assets)?;
    }

    Ok(())
}

/// Reads an asset whole, held to the document read limit: the size is
/// checked from metadata before any byte is read, so an oversized file is
/// refused with its path and the limit instead of filling memory.
#[cfg(not(target_arch = "wasm32"))]
fn read_asset_text(name: &str, path: &Path) -> Result<String, String> {
    use crate::pkg::read_limit::{human_bytes, read_document, ReadError};
    let failed = |err: &dyn std::fmt::Display| {
        format!(
            "Failed to read asset '{}' from '{}': {}",
            name,
            path.display(),
            err
        )
    };
    let bytes = read_document(path).map_err(|err| match err {
        ReadError::TooLarge(too_large) => format!(
            "Asset '{}' at '{}' is {} bytes, over the {} limit on assets; split it into \
             smaller asset files",
            name,
            path.display(),
            too_large.size,
            human_bytes(too_large.limit)
        ),
        ReadError::Io(err) => failed(&err),
    })?;
    String::from_utf8(bytes).map_err(|err| failed(&err))
}

#[cfg(target_arch = "wasm32")]
fn read_asset_text(name: &str, path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| {
        format!(
            "Failed to read asset '{}' from '{}': {}",
            name,
            path.display(),
            err
        )
    })
}

/// Returns true when a parsed JSON asset declares itself a `fugue.score.v1`
/// document via a top-level `schema` field, opting into load-time validation.
fn declares_score_schema(value: &serde_json::Value) -> bool {
    value
        .get("schema")
        .and_then(|schema| schema.as_str())
        .map(|schema| schema == crate::invention::score::SCORE_SCHEMA_V1)
        .unwrap_or(false)
}

fn resolve_asset_path(
    source_path: Option<&Path>,
    path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return Ok(candidate);
    }

    let Some(source_path) = source_path else {
        return Err(format!(
            "Relative asset path '{}' requires the parent invention to be loaded from a file",
            path
        )
        .into());
    };

    let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(base_dir.join(candidate))
}

pub(super) fn resolve_asset_refs(
    value: &serde_json::Value,
    assets: &HashMap<String, serde_json::Value>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| resolve_asset_refs(item, assets))
            .collect(),
        serde_json::Value::Object(map) => {
            if let Some(asset_name) = map.get("$asset") {
                let asset_name = asset_name.as_str().ok_or("$asset must be a string")?;
                if map.keys().any(|key| key != "$asset" && key != "path") {
                    return Err(
                        "asset reference objects may only contain '$asset' and 'path'".into(),
                    );
                }

                let asset = assets
                    .get(asset_name)
                    .ok_or_else(|| format!("Unknown asset reference '{}'", asset_name))?;

                let Some(path) = map.get("path") else {
                    return Ok(asset.clone());
                };
                let path = path
                    .as_str()
                    .ok_or("asset reference path must be a string")?;
                if !path.is_empty() && !path.starts_with('/') {
                    return Err(format!(
                        "asset reference path '{}' must be a JSON Pointer starting with '/'",
                        path
                    )
                    .into());
                }

                asset
                    .pointer(path)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "Asset reference '{}' does not contain JSON Pointer '{}'",
                            asset_name, path
                        )
                    })
                    .map_err(Into::into)
            } else {
                map.iter()
                    .map(|(key, value)| Ok((key.clone(), resolve_asset_refs(value, assets)?)))
                    .collect::<Result<serde_json::Map<String, serde_json::Value>, Box<dyn std::error::Error>>>()
                    .map(serde_json::Value::Object)
            }
        }
        _ => Ok(value.clone()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::pkg::read_limit::DOCUMENT_READ_LIMIT;

    #[test]
    fn an_asset_past_the_document_limit_is_refused_with_its_path_and_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        // Sparse, so the test costs no disk; refused from metadata, so it
        // costs no memory either.
        std::fs::File::create(dir.path().join("huge.json"))
            .unwrap()
            .set_len(DOCUMENT_READ_LIMIT + 1)
            .unwrap();
        let mut invention = Invention::from_json(
            r#"{"version": "1.0.0", "assets": {"score": {"path": "huge.json"}}, "modules": [], "connections": []}"#,
        )
        .unwrap();
        invention.source_path = Some(dir.path().join("piece.json"));

        let message = resolve_json_assets(&mut invention).unwrap_err().to_string();
        assert!(message.contains("huge.json"), "{message}");
        assert!(message.contains("16 MiB"), "{message}");
    }
}
