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
//! Verification is bound to the bytes a load consumes, not only to the
//! package id. Verifying a package records the SHA-256 of each file its
//! integrity hashed. A file is governed by where it lies, however its path
//! is spelled (the packages dir as configured, canonical, or through
//! another link): any file under the dir of a package the lock locks. A
//! governed file must be one of the locked version's verified files, with
//! those bytes, wherever a load reads it ([`check_bytes`]): as a governed ref
//! resolves to it, as a sample is decoded from it (and cached by its digest,
//! see `modules::sample_loading`), and as a document, JSON asset or sample
//! pack sidecar is read from it. A file the integrity skips (a symlink,
//! anything under `.git`), a file edited since, and a file of another
//! version of a locked package are refused.
//!
//! The governing lock is the running session's: a host changes it with
//! [`govern`] only when a load or reload commits. A candidate is built under
//! its own lock with [`govern_here`], which governs only the calling thread,
//! so the session's other threads (its scripts) keep the session's lock until
//! the candidate commits, and keep it for good if the candidate is refused.
//! A candidate built under another policy than the session's rebuilds what
//! may hold package content instead of keeping it (see
//! [`candidate_changes_policy`]): bytes a module loaded under the session's
//! policy were never verified against the candidate's.
//!
//! Resolution is control-thread work (asset loads decode and resample off the
//! audio thread), so the governing slot is a plain lock.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use semver::{Version, VersionReq};

use super::{LockError, Lockfile};
use crate::pkg::content::ContentRoots;
use crate::Invention;

/// A governing lockfile: read once when a frozen session loads, and the
/// locked packages whose bytes have been verified against it since.
#[derive(Debug)]
pub struct FrozenLock {
    path: PathBuf,
    lockfile: Lockfile,
    packages_dir: PathBuf,
    /// `packages_dir` with every link resolved, when it exists.
    canonical_packages: Option<PathBuf>,
    /// Each locked package verified so far, by id.
    verified: Mutex<BTreeMap<String, Arc<Verified>>>,
}

/// A locked package whose bytes hashed to the lock's integrity: its
/// directory relative to the packages dir, and the SHA-256 of each file that
/// hash covered, by normalized relative path.
#[derive(Debug)]
struct Verified {
    within: PathBuf,
    files: BTreeMap<String, String>,
}

/// The lock a frozen session's resolutions are governed by: `None` for none.
pub type Governance = Option<Arc<FrozenLock>>;

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
            canonical_packages: std::fs::canonicalize(packages_dir).ok(),
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
        let problems: Vec<String> = ids
            .iter()
            .filter(|id| self.lockfile.packages.contains_key(*id))
            .filter_map(|id| self.verify(id).err())
            .collect();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(LockError { problems }.into())
        }
    }

    /// The locked package `id`, verified against the lock's integrity once
    /// per lock. `id` must be locked.
    fn verify(&self, id: &str) -> Result<Arc<Verified>, String> {
        let cached = self.verified_package(id);
        if let Some(verified) = cached {
            return Ok(verified);
        }
        let locked = &self.lockfile.packages[id];
        let (dir, files) = super::fs_ops::verify_package_files(id, locked, &self.packages_dir)?;
        let within = dir
            .strip_prefix(&self.packages_dir)
            .map_err(|_| {
                format!(
                    "{id} is not installed under {}",
                    self.packages_dir.display()
                )
            })?
            .to_path_buf();
        let verified = Arc::new(Verified { within, files });
        self.verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string(), verified.clone());
        Ok(verified)
    }

    fn verified_package(&self, id: &str) -> Option<Arc<Verified>> {
        self.verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
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
        self.verify(id)
            .map_err(|problem| refused(format!("lockfile validation failed: {problem}")))?;
        Ok(Some(version))
    }

    /// Checks `file`, which a governed ref resolved to after [`Self::admit`]
    /// admitted its package: it must be one the verified integrity covered,
    /// and still hold the bytes verified.
    pub(crate) fn admit_file(
        &self,
        file: &Path,
        reference: &dyn std::fmt::Display,
    ) -> Result<(), String> {
        let expected = self
            .expected_digest(file)
            .unwrap_or_else(|| Err(self.uncovered(reference)))?;
        if digest_file(file)? != expected {
            return Err(self.changed(reference));
        }
        Ok(())
    }

    /// The SHA-256 a governed load must read at `file`, when it lies in the
    /// dir of a package this lock locks, however its path is spelled: the
    /// digest verified for it in the locked version, or a refusal when it is
    /// not one of that version's verified files (another version's, one the
    /// integrity skips) or the version does not verify. `None` for any other
    /// file, which this lock does not govern.
    pub(crate) fn expected_digest(&self, file: &Path) -> Option<Result<String, String>> {
        let relative = self.package_relative(file)?;
        let Some(Component::Normal(id)) = relative.components().next() else {
            return None;
        };
        let id = id.to_str()?;
        self.lockfile.packages.get(id)?;
        let refused = |why: String| {
            format!(
                "frozen load refused {}: {why} (lockfile {})",
                file.display(),
                self.path.display()
            )
        };
        Some(
            self.verify(id)
                .map_err(|problem| refused(format!("lockfile validation failed: {problem}")))
                .and_then(|package| {
                    let inside = relative.strip_prefix(&package.within).map_err(|_| {
                        let locked = &self.lockfile.packages[id].version;
                        refused(format!(
                            "it is not in {id}@{locked}, the version the lockfile locks"
                        ))
                    })?;
                    package
                        .digest_at(inside)
                        .map(str::to_string)
                        .ok_or_else(|| self.uncovered(&file.display()))
                }),
        )
    }

    /// `file` relative to the packages dir, when it lies in it: as spelled
    /// (the configured or the canonical spelling of the dir), else with its
    /// parent's links resolved. Only the parent is resolved, so a link
    /// inside a package still names a file the integrity skipped.
    fn package_relative(&self, file: &Path) -> Option<PathBuf> {
        let spellings = std::iter::once(&self.packages_dir).chain(&self.canonical_packages);
        for dir in spellings {
            if let Ok(relative) = file.strip_prefix(dir) {
                return Some(relative.to_path_buf());
            }
        }
        let canonical = std::fs::canonicalize(file.parent()?)
            .ok()?
            .join(file.file_name()?);
        let relative = canonical
            .strip_prefix(self.canonical_packages.as_ref()?)
            .ok()?;
        Some(relative.to_path_buf())
    }

    /// Whether `other` governs as this lock does: the same lockfile, the
    /// same content, over the same packages dir.
    fn same_policy(&self, other: &FrozenLock) -> bool {
        self.path == other.path
            && self.lockfile == other.lockfile
            && same_dir(&self.packages_dir, &other.packages_dir)
    }

    fn uncovered(&self, what: &dyn std::fmt::Display) -> String {
        format!(
            "frozen load refused {what}: it is not among the files the lockfile's integrity \
             covers, such as a symlink or a file under .git (lockfile {})",
            self.path.display()
        )
    }

    /// The refusal for a file whose bytes changed since they were verified.
    pub(crate) fn changed(&self, what: &dyn std::fmt::Display) -> String {
        format!(
            "frozen load refused {what}: its bytes changed since the lockfile verified them \
             (lockfile {}). Update the lockfile with `fyug install`, or load without --frozen",
            self.path.display()
        )
    }
}

impl Verified {
    /// The verified digest of the file at `relative` inside the package,
    /// when it is one of the files the integrity hashed.
    fn digest_at(&self, relative: &Path) -> Option<&str> {
        let mut parts = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => parts.push(part.to_str()?),
                _ => return None,
            }
        }
        self.files.get(&parts.join("/")).map(String::as_str)
    }
}

/// SHA-256 of the file at `path`, read under the same bound as the integrity
/// hash.
fn digest_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    crate::pkg::read_limit::HashedFile::open(path)
        .and_then(|file| file.stream(|chunk| hasher.update(chunk)))
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    Ok(crate::hex::lower_hex(&hasher.finalize()))
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

static GOVERNING: RwLock<Governance> = RwLock::new(None);

thread_local! {
    /// The lock governing this thread's resolutions while it builds a
    /// candidate ([`govern_here`]), in place of the session's.
    static HERE: RefCell<Option<Governance>> = const { RefCell::new(None) };
}

/// Makes `lock` the lockfile that governs the running session's package
/// resolution in this process (`None`: nothing governs), returning the one
/// it replaces. A host calls it when a load or reload commits.
pub fn govern(lock: Governance) -> Governance {
    std::mem::replace(
        &mut *GOVERNING.write().unwrap_or_else(PoisonError::into_inner),
        lock,
    )
}

/// Runs `body` with `lock` governing the resolutions this thread makes, and
/// only this thread's: a candidate document builds under its own lock while
/// the running session, its scripts included, keeps the session's.
pub fn govern_here<T>(lock: Governance, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<Governance>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HERE.with(|here| *here.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(HERE.with(|here| here.borrow_mut().replace(lock)));
    body()
}

/// The lockfile that governs package resolution on this thread now, if any:
/// the candidate's while [`govern_here`] builds one, otherwise the session's.
pub fn governing() -> Governance {
    if let Some(here) = HERE.with(|here| here.borrow().clone()) {
        return here;
    }
    GOVERNING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Checks bytes a load read from `path` (a document, a JSON asset, a sample
/// pack sidecar) against the lockfile that governs this thread: when `path`
/// lies in a package it locks, they must be the bytes verified for that file
/// of the locked version. Anything else passes.
pub(crate) fn check_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let Some(lock) = governing() else {
        return Ok(());
    };
    let Some(expected) = lock.expected_digest(path) else {
        return Ok(());
    };
    if crate::hex::lower_hex(&Sha256::digest(bytes)) != expected? {
        return Err(lock.changed(&path.display()));
    }
    Ok(())
}

/// Whether the candidate this thread builds ([`govern_here`]) is governed
/// otherwise than the running session: another lockfile or none, or other
/// locked content. What the session's modules hold was then loaded under
/// another policy, and a candidate keeps none of them that may hold package
/// content. `false` when no candidate is being built here.
pub(crate) fn candidate_changes_policy() -> bool {
    let Some(candidate) = HERE.with(|here| here.borrow().clone()) else {
        return false;
    };
    let session = GOVERNING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match (&candidate, &session) {
        (None, None) => false,
        (Some(candidate), Some(session)) => {
            !(Arc::ptr_eq(candidate, session) || candidate.same_policy(session))
        }
        _ => true,
    }
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
