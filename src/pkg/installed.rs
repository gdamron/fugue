//! Installed packages as a resolution source, under the governing lock.
//!
//! [`InstalledPackages`] answers [`PackageProvider`] from a packages
//! directory alone (either layout `crate::pkg::layout` describes), so
//! resolving against it reads only what is installed and never reaches a
//! registry or the network. While a frozen session's lockfile governs the
//! calling thread (see [`crate::pkg::lock::govern`]), a package it locks
//! offers exactly its locked version, verified against the lock's integrity,
//! and every file read from a locked package must hold the bytes verified.

use std::error::Error;
use std::path::{Path, PathBuf};

use semver::Version;

use super::lock::{check_bytes, Governance};
use super::resolve::PackageProvider;
use super::{layout, PackageManifest};

/// The packages under one packages directory, read under the lock that
/// governed when this was made.
pub(crate) struct InstalledPackages<'a> {
    packages_dir: &'a Path,
    lock: Governance,
}

impl<'a> InstalledPackages<'a> {
    /// Packages under `packages_dir`, governed as the calling thread is now.
    pub(crate) fn new(packages_dir: &'a Path) -> Self {
        Self {
            packages_dir,
            lock: super::lock::governing(),
        }
    }

    /// The governing lockfile's path, when one governs.
    pub(crate) fn lockfile(&self) -> Option<&Path> {
        self.lock.as_deref().map(|lock| lock.path())
    }
}

impl PackageProvider for InstalledPackages<'_> {
    /// The installed version's directory.
    type Acquired = PathBuf;

    fn available_versions(&self, id: &str) -> Result<Vec<Version>, Box<dyn Error>> {
        let installed = layout::available(self.packages_dir, id)?.versions();
        let Some(lock) = &self.lock else {
            return Ok(installed);
        };
        let Some(locked) = lock.locked_version(self.packages_dir, id, &id)? else {
            return Ok(installed);
        };
        if !installed.contains(&locked) {
            return Err(format!(
                "the lockfile locks {id}@{locked}, which is not installed in {} (lockfile {})",
                self.packages_dir.display(),
                lock.path().display()
            )
            .into());
        }
        Ok(vec![locked])
    }

    fn acquire(
        &self,
        id: &str,
        version: &Version,
    ) -> Result<(PackageManifest, PathBuf), Box<dyn Error>> {
        let dir = layout::available(self.packages_dir, id)?.dir(self.packages_dir, id, version);
        let manifest = read_manifest(&dir)?;
        if manifest.id != id || manifest.version != version.to_string() {
            return Err(format!(
                "{} declares {}@{}, not {id}@{version}",
                dir.join("fugue.pkg.json").display(),
                manifest.id,
                manifest.version
            )
            .into());
        }
        Ok((manifest, dir))
    }
}

/// Read the bytes of `path` for a load: bounded like any document and, when
/// it lies in a package the governing lock locks, the bytes it verified.
pub(crate) fn read_governed(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = super::read_limit::read_document(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    check_bytes(path, &bytes)?;
    Ok(bytes)
}

/// Parse and validate the manifest in `package_dir`, read as [`read_governed`].
pub(crate) fn read_manifest(package_dir: &Path) -> Result<PackageManifest, String> {
    let path = package_dir.join("fugue.pkg.json");
    let bytes = read_governed(&path)?;
    let manifest: PackageManifest =
        serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    super::validate(&manifest).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(manifest)
}
