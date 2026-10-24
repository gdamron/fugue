//! Live writes to an audio asset control, resolved as the loader resolves
//! the document's assets.
//!
//! The loader resolves a module's relative asset path against the
//! invention's directory before the module is built (see
//! [`resolve_audio_assets`](super::resolve_audio_assets)). A live write to
//! an asset control (a sample player's `asset`, a kit slot's or instrument
//! zone's `asset.N`) is resolved here by the same rule, so the file it plays
//! now is the file the saved document loads. The document keeps the value as
//! written; only the module and its stored config get the resolved path, as
//! the loader gives them. Package refs, URLs and rooted paths pass through,
//! for the module to resolve as it does at load.

use std::path::Path;

use super::is_asset_control;
use crate::ControlValue;

/// `value` as the module should load it, for a live write to control `key`
/// of an invention loaded from `source_path`.
///
/// A relative path in a write to an asset control joins the invention's
/// directory. With no invention file there is nothing to resolve it
/// against, so the write is refused rather than loaded against the
/// process's working directory, which a saved document would not reproduce.
/// Anything else comes back unchanged.
pub(crate) fn resolve_asset_write(
    source_path: Option<&Path>,
    key: &str,
    value: &ControlValue,
) -> Result<ControlValue, String> {
    let ControlValue::String(text) = value else {
        return Ok(value.clone());
    };
    if !is_asset_control(key) || !is_relative_path(text) {
        return Ok(value.clone());
    }
    match source_path.and_then(Path::parent) {
        Some(dir) => Ok(ControlValue::String(join(dir, text))),
        None => Err(format!(
            "'{key}' was given the relative path '{text}', but this invention has no \
             file to resolve it against; give an absolute path, or save the invention \
             and write the path relative to its file"
        )),
    }
}

/// [`resolve_asset_write`] for a write already checked at its front door
/// (a commit, a reload): a relative path with nothing to resolve it against
/// is kept as written.
pub(crate) fn resolve_checked_asset_write(
    source_path: Option<&Path>,
    key: &str,
    value: &ControlValue,
) -> ControlValue {
    resolve_asset_write(source_path, key, value).unwrap_or_else(|_| value.clone())
}

/// Whether the loader would join `text` to the invention's directory: a
/// local path that is not rooted, a URL or a package ref.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn is_relative_path(text: &str) -> bool {
    crate::pkg::PackageAudioRef::parse(text).is_none() && super::non_wasm::is_relative_local(text)
}

/// On wasm the loader joins nothing, so neither does a live write.
#[cfg(target_arch = "wasm32")]
pub(crate) fn is_relative_path(_text: &str) -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
fn join(dir: &Path, text: &str) -> String {
    super::non_wasm::resolve_local(text, Some(dir))
}

#[cfg(target_arch = "wasm32")]
fn join(_dir: &Path, text: &str) -> String {
    text.to_string()
}
