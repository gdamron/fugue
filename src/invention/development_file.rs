//! Writing a development file's layout: the arrangement of a development
//! the piece loaded from a file, saved in that file (see `move_modules`'s
//! `development` in `docs/apply-edits.md`).
//!
//! Only the `layout` member of the arranged definition changes in the
//! file's text; every other byte stays as it was. The file is refused when
//! it is never written here (in the package cache, or locked by a frozen
//! lockfile), cannot be written, or no longer holds what the piece loaded.
//! The new text is staged beside the file with no lock held, then replaces
//! the old in one rename, so a reader never sees half of it. All of it runs
//! on the calling control thread.

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

/// Writes `layout` as the layout of the definition `within` names in the
/// development file at `path`, staging and then placing it at once (see
/// [`stage_layout`]).
#[cfg(test)]
pub(crate) fn write_layout(
    path: &Path,
    within: &[String],
    loaded: &Invention,
    layout: &Layout,
) -> Result<(), NotWritten> {
    stage_layout(path, within, loaded, layout)?.place()
}

/// Stages `layout` as the layout of the definition `within` names (each
/// name an inline declaration inside the one before it; none, the file's
/// own) in the development file at `path`: the new text is written and
/// synced beside the file, ready for [`Staged::place`] to rename over it.
/// `loaded` is the file's own definition as the piece loaded it: the file
/// must still hold exactly it. Nothing here holds a lock, so the slow part
/// of a write (the sync) delays no other control-side writer.
pub(crate) fn stage_layout(
    path: &Path,
    within: &[String],
    loaded: &Invention,
    layout: &Layout,
) -> Result<Staged, NotWritten> {
    let shown = path.display();
    if let Some(reason) = never_written(path) {
        return Err(NotWritten::ReadOnly(format!(
            "the development file '{shown}' is never written here: {reason}"
        )));
    }
    let failed = |why: String| failed(path, why);
    // Opened for writing, not truncated: a file that refuses writes is
    // refused before anything is made beside it.
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| failed(format!("cannot be written: {error}")))?;
    let bytes = fs::read(path).map_err(|error| failed(format!("cannot be read: {error}")))?;
    if super::file::is_compressed(&bytes) {
        return Err(failed(
            "is compressed (gzip), and a layout is arranged only in plain JSON; save it \
             uncompressed (gunzip -c it to a new file), point the piece at that file and \
             load it again to arrange it"
                .to_string(),
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| failed("cannot be read: it is not UTF-8 text".to_string()))?;
    let on_disk = Invention::from_json(&text).map_err(|_| changed(path))?;
    if on_disk != without_source(loaded) {
        return Err(changed(path));
    }
    let mut expected = on_disk;
    let target = inline_definition_mut(&mut expected, within).ok_or_else(|| changed(path))?;
    target.layout = layout.clone();
    let written = splice::with_layout(&text, within, layout)
        .ok_or_else(|| failed("could not be read as a development document".to_string()))?;
    // The text changed only where the layout is, or it is not written.
    if Invention::from_json(&written).ok().as_ref() != Some(&expected) {
        return Err(failed(
            "could not be arranged without changing more than its layout".to_string(),
        ));
    }
    Staged::new(path, text, &written).map_err(|error| failed(format!("cannot be written: {error}")))
}

fn failed(path: &Path, why: String) -> NotWritten {
    NotWritten::Failed(format!("the development file '{}' {why}", path.display()))
}

fn changed(path: &Path) -> NotWritten {
    failed(
        path,
        "changed on disk since the piece loaded it; reload the piece to take the change, \
         then arrange it again"
            .to_string(),
    )
}

/// `document` as parsed from text, which carries no source path.
fn without_source(document: &Invention) -> Invention {
    let mut document = document.clone();
    document.source_path = None;
    document
}

/// A development file's new text, written and synced beside it in its own
/// directory with its permissions, waiting to be renamed over it. Dropped
/// without being placed, the staged file is removed.
pub(crate) struct Staged {
    path: PathBuf,
    beside: PathBuf,
    /// What the file held when it was staged.
    was: String,
    placed: bool,
}

impl Staged {
    fn new(path: &Path, was: String, text: &str) -> std::io::Result<Staged> {
        let permissions = fs::metadata(path)?.permissions();
        let beside = beside(path);
        // Made here or not at all, so the file removed on drop is this
        // one's own.
        let mut file = (fs::OpenOptions::new().write(true).create_new(true)).open(&beside)?;
        let staged = Staged {
            path: path.to_owned(),
            beside,
            was,
            placed: false,
        };
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(&staged.beside, permissions)?;
        Ok(staged)
    }

    /// Renames the staged text over the file, in one rename, unless the
    /// file no longer holds what it held when staged: another writer saved
    /// it meanwhile, and it is refused as changed on disk.
    pub(crate) fn place(self) -> Result<(), NotWritten> {
        let path = self.path.clone();
        match self.rename() {
            Ok(true) => Ok(()),
            Ok(false) => Err(changed(&path)),
            Err(error) => Err(failed(&path, format!("cannot be written: {error}"))),
        }
    }

    /// `false`, with nothing replaced, when the file changed since it was
    /// staged. (No portable rename compares first, so a save in the moment
    /// between this read and the rename is still lost.)
    fn rename(mut self) -> std::io::Result<bool> {
        if fs::read_to_string(&self.path)? != self.was {
            return Ok(false);
        }
        fs::rename(&self.beside, &self.path)?;
        self.placed = true;
        Ok(true)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.placed {
            let _ = fs::remove_file(&self.beside);
        }
    }
}

/// Replaces the file at `path`, which held `was`, with `text`: staged
/// beside it, then renamed over it. `false`, with nothing replaced, when it
/// no longer holds `was` just before the rename.
#[cfg(test)]
fn replace(path: &Path, was: &str, text: &str) -> std::io::Result<bool> {
    Staged::new(path, was.to_string(), text)?.rename()
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
