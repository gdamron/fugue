//! The lockfile that governs a frozen session, enforced where package refs
//! resolve.
//!
//! A frozen load verifies the packages an invention uses before it builds
//! (see [`Lockfile::verify_first_for_invention`]), but every loader used to
//! pick its own version: the document loader from a lockfile beside the
//! invention, a control write from no lockfile at all, a development `ref`
//! from the exact version it names. So a frozen session could load a version
//! the governing lockfile never locked, and so never verified.
//!
//! A host makes a verified [`FrozenLock`] the governing one with [`govern`]
//! for as long as a frozen session runs. From then on
//! [`crate::pkg::resolve_package_asset`], which every load path reaches
//! (document and development loads, reloads, timed reloads, control writes,
//! scripts, development refs and the assets inside them), resolves a package
//! the lock locks to exactly its locked version, and only after that
//! version's bytes have been verified against the lock's integrity. A ref
//! the locked version does not satisfy, a locked version that is not
//! present, and bytes that do not hash to the locked integrity are all
//! refused. A package the lockfile does not lock is not governed.
//!
//! Resolution is control-thread work (asset loads decode and resample off the
//! audio thread), so the governing slot is a plain lock.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use semver::{Version, VersionReq};

use super::Lockfile;
use crate::pkg::content::ContentRoots;
use crate::Invention;

/// A governing lockfile: read once when a frozen session loads, and the
/// locked packages whose bytes have been verified against it since.
#[derive(Debug)]
pub struct FrozenLock {
    path: PathBuf,
    lockfile: Lockfile,
    packages_dir: PathBuf,
    verified: Mutex<BTreeSet<String>>,
}

impl FrozenLock {
    /// The first existing lockfile among `candidates`, governing packages
    /// under `packages_dir`. `None` when there is none: a frozen load only
    /// enforces a lockfile that is present.
    pub fn find(
        candidates: &[PathBuf],
        packages_dir: &Path,
    ) -> Result<Option<Self>, Box<dyn Error>> {
        let Some(path) = candidates.iter().find(|path| path.is_file()) else {
            return Ok(None);
        };
        Ok(Some(Self {
            lockfile: Lockfile::read(path)?,
            path: path.clone(),
            packages_dir: packages_dir.to_path_buf(),
            verified: Mutex::default(),
        }))
    }

    /// The lockfile's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Verifies up front the packages `invention` uses and their locked
    /// dependencies (every locked package when that set cannot be known),
    /// so a tampered package fails the load before anything builds. Those
    /// packages are then not hashed again when they resolve.
    pub fn verify_invention(&self, invention: &Invention) -> Result<(), Box<dyn Error>> {
        let used = ContentRoots::from_environment().ok().and_then(|roots| {
            ContentRoots {
                packages: self.packages_dir.clone(),
                ..roots
            }
            .used_packages(invention)
        });
        let ids = match used {
            Some(used) => self
                .lockfile
                .dependency_closure(used.iter().map(String::as_str)),
            None => self.lockfile.packages.keys().cloned().collect(),
        };
        self.lockfile
            .verify_ids(&self.packages_dir, ids.iter().map(String::as_str))?;
        self.verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(
                ids.into_iter()
                    .filter(|id| self.lockfile.packages.contains_key(id)),
            );
        Ok(())
    }

    /// The version a resolution of `reference` (package `id`, requirement
    /// `requirement`) under `packages_dir` must use, its bytes verified.
    /// `Ok(None)` when this lock does not govern it: another packages dir,
    /// or a package it does not lock.
    pub(crate) fn admit(
        &self,
        packages_dir: &Path,
        id: &str,
        requirement: &VersionReq,
        reference: &dyn std::fmt::Display,
    ) -> Result<Option<Version>, String> {
        if !same_dir(packages_dir, &self.packages_dir) {
            return Ok(None);
        }
        let Some(locked) = self.lockfile.packages.get(id) else {
            return Ok(None);
        };
        let refused = |why: String| {
            format!(
                "frozen load refused {reference}: {why} (lockfile {}). Update the lockfile \
                 with `fyug install`, or load without --frozen",
                self.path.display()
            )
        };
        let version = Version::parse(&locked.version)
            .map_err(|err| refused(format!("{id} is locked to invalid version: {err}")))?;
        if !requirement.matches(&version) {
            return Err(refused(format!(
                "the lockfile locks {id}@{version}, which does not satisfy {requirement}"
            )));
        }
        let verified = |ids: &BTreeSet<String>| ids.contains(id);
        if verified(&self.verified.lock().unwrap_or_else(PoisonError::into_inner)) {
            return Ok(Some(version));
        }
        if let Some(problem) = super::fs_ops::verify_package(id, locked, &self.packages_dir) {
            return Err(refused(format!("lockfile validation failed: {problem}")));
        }
        self.verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string());
        Ok(Some(version))
    }
}

/// Whether two spellings name one packages dir. Equal as written in every
/// host (both come from `default_packages_dir`), so the filesystem is asked
/// only when they differ.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

static GOVERNING: RwLock<Option<Arc<FrozenLock>>> = RwLock::new(None);

/// Makes `lock` the lockfile that governs package resolution in this
/// process (`None`: nothing governs), returning the one it replaces so a
/// host can put it back when a load fails.
pub fn govern(lock: Option<Arc<FrozenLock>>) -> Option<Arc<FrozenLock>> {
    std::mem::replace(
        &mut *GOVERNING.write().unwrap_or_else(PoisonError::into_inner),
        lock,
    )
}

/// The lockfile that governs package resolution now, if any.
pub fn governing() -> Option<Arc<FrozenLock>> {
    GOVERNING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Test-only: run `body` with `lock` governing, serialized across the test
/// binary because the governing slot is process-wide. Restores the previous
/// lock afterwards, even on a panic.
#[cfg(test)]
pub(crate) fn with_governing(lock: FrozenLock, body: impl FnOnce()) {
    static SERIAL: Mutex<()> = Mutex::new(());
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let previous = govern(Some(Arc::new(lock)));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    govern(previous);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[cfg(test)]
mod tests;
