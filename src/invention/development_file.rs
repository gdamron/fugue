//! Writing a development file's layout: the arrangement of a development
//! the piece loaded from a file, saved in that file (see `move_modules`'s
//! `development` in `docs/apply-edits.md`).
//!
//! Only the `layout` member of the arranged definition changes in the
//! file's text; every other byte stays as it was. The file is refused when
//! it is never written here (in the package cache, or locked by a frozen
//! lockfile), cannot be written, or no longer holds what the piece loaded.
//! The new text replaces the old in one rename, so a reader never sees half
//! of it. All of it runs on the calling control thread.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::layout::Layout;
use super::reload::inline_definition_mut;
use crate::Invention;

mod splice;

/// Why a development file's layout was not written.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotWritten {
    /// The file is never written here.
    ReadOnly(String),
    /// The file could not be written, or changed on disk since it loaded.
    Failed(String),
}

/// Why the development file at `path` is never written here, if it is not:
/// it lies in the package cache, where installed packages live, or a frozen
/// lockfile governing the session locks it.
pub(crate) fn never_written(path: &Path) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(lock) = crate::pkg::lock::governing() {
            if lock.expected_digest(path).is_some() {
                return Some(
                    "its file is in a package the session's frozen lockfile locks".to_string(),
                );
            }
        }
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if let Ok(packages) = crate::pkg::default_packages_dir() {
            if canonical(path).starts_with(canonical(&packages)) {
                return Some("it comes from an installed package".to_string());
            }
        }
    }
    let _ = path;
    None
}

/// Writes `layout` as the layout of the definition `within` names (each
/// name an inline declaration inside the one before it; none, the file's
/// own) in the development file at `path`. `loaded` is the file's own
/// definition as the piece loaded it: the file must still hold exactly it.
pub(crate) fn write_layout(
    path: &Path,
    within: &[String],
    loaded: &Invention,
    layout: &Layout,
) -> Result<(), NotWritten> {
    let shown = path.display();
    if let Some(reason) = never_written(path) {
        return Err(NotWritten::ReadOnly(format!(
            "the development file '{shown}' is never written here: {reason}"
        )));
    }
    let failed = |why: String| NotWritten::Failed(format!("the development file '{shown}' {why}"));
    // Opened for writing, not truncated: a file that refuses writes is
    // refused before anything is made beside it.
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| failed(format!("cannot be written: {error}")))?;
    let text =
        fs::read_to_string(path).map_err(|error| failed(format!("cannot be read: {error}")))?;
    let changed = || {
        failed("changed on disk since the piece loaded it; reload the piece to take the change, then arrange it again".to_string())
    };
    let on_disk = Invention::from_json(&text).map_err(|_| changed())?;
    if on_disk != without_source(loaded) {
        return Err(changed());
    }
    let mut expected = on_disk;
    let target = inline_definition_mut(&mut expected, within).ok_or_else(changed)?;
    target.layout = layout.clone();
    let written = splice::with_layout(&text, within, layout)
        .ok_or_else(|| failed("could not be read as a development document".to_string()))?;
    // The text changed only where the layout is, or it is not written.
    if Invention::from_json(&written).ok().as_ref() != Some(&expected) {
        return Err(failed(
            "could not be arranged without changing more than its layout".to_string(),
        ));
    }
    match replace(path, &text, &written) {
        Ok(true) => Ok(()),
        Ok(false) => Err(changed()),
        Err(error) => Err(failed(format!("cannot be written: {error}"))),
    }
}

/// `document` as parsed from text, which carries no source path.
fn without_source(document: &Invention) -> Invention {
    let mut document = document.clone();
    document.source_path = None;
    document
}

/// Replaces the file at `path`, which held `was`, with `text` in one
/// rename in its own directory, keeping its permissions: written beside it
/// first, then moved over it. `false`, with nothing replaced, when the file
/// no longer holds `was` just before the rename: another writer saved it
/// meanwhile. (No portable rename compares first, so a save in the moment
/// between that read and the rename is still lost.)
fn replace(path: &Path, was: &str, text: &str) -> std::io::Result<bool> {
    let permissions = fs::metadata(path)?.permissions();
    let beside = beside(path);
    // Made here or not at all, so the file removed below is this one's own.
    let mut file = (fs::OpenOptions::new().write(true).create_new(true)).open(&beside)?;
    let written = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(&beside, permissions)?;
        if fs::read_to_string(path)? != was {
            return Ok(false);
        }
        fs::rename(&beside, path)?;
        Ok(true)
    })();
    if !matches!(written, Ok(true)) {
        let _ = fs::remove_file(&beside);
    }
    written
}

/// A hidden file name beside `path`, unique to this replacement: the
/// process and a count, so replacements of one file never share one.
fn beside(path: &Path) -> PathBuf {
    static MADE: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let name = format!(
        ".{}.{}.{}.arranging",
        name.unwrap_or_default(),
        std::process::id(),
        MADE.fetch_add(1, Ordering::Relaxed)
    );
    path.with_file_name(name)
}

#[cfg(test)]
mod tests;
