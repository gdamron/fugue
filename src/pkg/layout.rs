//! Where a package's versions live under a packages directory.
//!
//! A packages directory (`FUGUE_PACKS_DIR`, or `~/.fugue/packs`) holds each
//! package in one of two layouts:
//!
//! - **Installed**, what `fugue install` stages: `<packs>/<id>/<version>/`,
//!   one directory per installed version.
//! - **Source checkout**, a pack repository's `packs/` directory:
//!   `<packs>/<id>/fugue.pkg.json`, a single version whose number is the
//!   manifest's `version`. Pointing `FUGUE_PACKS_DIR` at a checkout loads its
//!   packs without staging them.
//!
//! The rules, shared by asset and content resolution, catalog listing and
//! lockfile verification (the package-management list and info calls report
//! installed versions only, since a checkout is not an install):
//!
//! - A source checkout offers exactly its manifest's version. Requirements
//!   and lockfile pins match it as they would an installed version, so an
//!   exact pin (`=2026.10.2`) needs that exact manifest version.
//! - Its manifest must parse and validate, and its `id` must equal the
//!   directory name. Otherwise the package is refused, never skipped.
//! - Every entry under it must be a regular file or directory that integrity
//!   hashing covers: no symlinks (the package directory included) and no
//!   `.git`. An install is a staged copy; a checkout is read in place, so
//!   anything the hash skips could change playback without changing identity.
//! - A package directory holding both layouts is ambiguous and refused:
//!   neither layout wins. Remove one.
//! - Content identity is unchanged. A package's bytes are hashed from the
//!   directory this module returns, so lockfile and frozen verification
//!   compare a source checkout's bytes exactly as they would an install's.

use semver::Version;
use std::path::{Path, PathBuf};

/// The versions of one package under a packages directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Available {
    /// Installed version directories (possibly none).
    Installed(Vec<Version>),
    /// A source checkout offering its manifest's single version.
    Source(Version),
}

impl Available {
    /// Every version on offer.
    pub(crate) fn versions(&self) -> Vec<Version> {
        match self {
            Self::Installed(versions) => versions.clone(),
            Self::Source(version) => vec![version.clone()],
        }
    }

    /// The directory holding `version` (which must be one on offer).
    pub(crate) fn dir(&self, packages_dir: &Path, id: &str, version: &Version) -> PathBuf {
        match self {
            Self::Installed(_) => packages_dir.join(id).join(version.to_string()),
            Self::Source(_) => packages_dir.join(id),
        }
    }
}

/// Whether `<packages_dir>/<id>` is a source checkout (it holds
/// `fugue.pkg.json` directly). Installers must not add version directories
/// beside one: that would make the package's layout ambiguous, which
/// resolution refuses.
pub fn is_source_checkout(packages_dir: &Path, id: &str) -> bool {
    packages_dir.join(id).join("fugue.pkg.json").is_file()
}

/// The versions of `id` under `packages_dir`, in whichever layout holds it.
/// A missing package directory offers no versions.
pub(crate) fn available(packages_dir: &Path, id: &str) -> Result<Available, String> {
    let package_dir = packages_dir.join(id);
    let installed = installed_versions(&package_dir);
    let manifest_path = package_dir.join("fugue.pkg.json");
    if !is_source_checkout(packages_dir, id) {
        return Ok(Available::Installed(installed));
    }
    if !installed.is_empty() {
        return Err(format!(
            "{} holds both installed versions ({id}/<version>/) and a source checkout \
             ({id}/fugue.pkg.json); remove one so the package resolves unambiguously",
            package_dir.display()
        ));
    }
    let mut entries = 0;
    every_byte_hashed(&package_dir, 0, &mut entries)?;
    let manifest = crate::pkg::parse_path(&manifest_path)
        .map_err(|e| format!("invalid source checkout {}: {e}", manifest_path.display()))?;
    if manifest.id != id {
        return Err(format!(
            "source checkout {} declares id {}, not {id}",
            manifest_path.display(),
            manifest.id
        ));
    }
    let version = Version::parse(&manifest.version).map_err(|e| {
        format!(
            "source checkout {} has invalid version {}: {e}",
            manifest_path.display(),
            manifest.version
        )
    })?;
    Ok(Available::Source(version))
}

/// The directory holding exactly `id@version`, in either layout, or `None`
/// when that version is not present.
pub(crate) fn version_dir(
    packages_dir: &Path,
    id: &str,
    version: &str,
) -> Result<Option<PathBuf>, String> {
    let Ok(version) = Version::parse(version) else {
        return Ok(None);
    };
    let available = available(packages_dir, id)?;
    Ok(available
        .versions()
        .contains(&version)
        .then(|| available.dir(packages_dir, id, &version)))
}

/// Refuse a source checkout holding anything `compute_integrity` skips, so
/// its hash covers every byte a load can read. Bounded like content walks.
fn every_byte_hashed(dir: &Path, depth: usize, entries: &mut usize) -> Result<(), String> {
    let refuse = |path: &Path, why: &str| {
        Err(format!(
            "source checkout entry {} {why}; integrity hashing would not cover it",
            path.display()
        ))
    };
    let metadata = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if metadata.file_type().is_symlink() {
        return refuse(dir, "is a symlink");
    }
    if depth > 64 {
        return refuse(dir, "nests more than 64 directories");
    }
    let listing = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in listing {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        *entries += 1;
        if *entries > 10_000 {
            return refuse(dir, "holds more than 10000 entries");
        }
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_symlink() {
            return refuse(&path, "is a symlink");
        }
        if kind.is_dir() {
            if entry.file_name() == ".git" {
                return refuse(&path, "is a .git directory");
            }
            every_byte_hashed(&path, depth + 1, entries)?;
        } else if !kind.is_file() {
            return refuse(&path, "is not a regular file");
        }
    }
    Ok(())
}

/// Installed version directories (names that parse as semver).
fn installed_versions(package_dir: &Path) -> Vec<Version> {
    let Ok(entries) = std::fs::read_dir(package_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            Version::parse(entry.file_name().to_str()?).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn manifest(dir: &Path, id: &str, version: &str) {
        fs::create_dir_all(dir).unwrap();
        let json = serde_json::json!({
            "id": id,
            "version": version,
            "kind": "development",
            "license": "MIT",
            "authors": [{"name": "Test"}],
            "targets": ["in-graph-agent"],
            "entry": {"development": "voice.json"},
        });
        fs::write(dir.join("fugue.pkg.json"), json.to_string()).unwrap();
    }

    #[test]
    fn installed_layout_lists_version_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        manifest(&tmp.path().join("fugue.a.b/1.0.0"), "fugue.a.b", "1.0.0");
        manifest(&tmp.path().join("fugue.a.b/1.2.0"), "fugue.a.b", "1.2.0");
        let mut versions = available(tmp.path(), "fugue.a.b").unwrap().versions();
        versions.sort();
        assert_eq!(versions, vec![Version::new(1, 0, 0), Version::new(1, 2, 0)]);
        assert!(!is_source_checkout(tmp.path(), "fugue.a.b"));
        assert_eq!(
            available(tmp.path(), "fugue.missing.pkg").unwrap(),
            Available::Installed(Vec::new())
        );
    }

    #[test]
    fn source_checkout_offers_its_manifest_version() {
        let tmp = tempfile::tempdir().unwrap();
        manifest(&tmp.path().join("fugue.a.b"), "fugue.a.b", "2026.10.2");
        let found = available(tmp.path(), "fugue.a.b").unwrap();
        assert_eq!(found, Available::Source(Version::new(2026, 10, 2)));
        assert!(crate::pkg::is_source_checkout(tmp.path(), "fugue.a.b"));
        assert_eq!(
            version_dir(tmp.path(), "fugue.a.b", "2026.10.2").unwrap(),
            Some(tmp.path().join("fugue.a.b"))
        );
        assert_eq!(
            version_dir(tmp.path(), "fugue.a.b", "2026.10.1").unwrap(),
            None
        );
    }

    #[test]
    fn oversized_source_manifest_is_refused_before_reading() {
        let tmp = tempfile::tempdir().unwrap();
        manifest(&tmp.path().join("fugue.a.b"), "fugue.a.b", "2026.10.2");
        let refused = crate::pkg::read_limit::test_override::with_document_limit(16, || {
            available(tmp.path(), "fugue.a.b").unwrap_err()
        });
        assert!(refused.contains("limited to"), "{refused}");
    }

    #[test]
    fn source_checkout_with_wrong_id_or_bad_manifest_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        manifest(&tmp.path().join("fugue.a.b"), "fugue.other.pkg", "1.0.0");
        let error = available(tmp.path(), "fugue.a.b").unwrap_err();
        assert!(error.contains("declares id fugue.other.pkg"), "{error}");

        fs::create_dir_all(tmp.path().join("fugue.bad.pkg")).unwrap();
        fs::write(tmp.path().join("fugue.bad.pkg/fugue.pkg.json"), b"{").unwrap();
        let error = available(tmp.path(), "fugue.bad.pkg").unwrap_err();
        assert!(error.contains("invalid source checkout"), "{error}");
    }

    #[test]
    fn mixed_layouts_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        manifest(&tmp.path().join("fugue.a.b"), "fugue.a.b", "1.0.0");
        manifest(&tmp.path().join("fugue.a.b/1.0.0"), "fugue.a.b", "1.0.0");
        let error = available(tmp.path(), "fugue.a.b").unwrap_err();
        assert!(error.contains("both installed versions"), "{error}");
        assert!(version_dir(tmp.path(), "fugue.a.b", "1.0.0").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn source_checkout_with_unhashed_entries_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let packs = tmp.path().join("packs");
        let package = packs.join("fugue.a.b");
        manifest(&package, "fugue.a.b", "1.0.0");
        fs::write(package.join("a.wav"), b"a").unwrap();
        assert!(available(&packs, "fugue.a.b").is_ok());

        // A link inside the package: retargeting it would change playback
        // without changing the hash, which skips links.
        std::os::unix::fs::symlink(package.join("a.wav"), package.join("b.wav")).unwrap();
        let error = available(&packs, "fugue.a.b").unwrap_err();
        assert!(error.contains("b.wav is a symlink"), "{error}");
        fs::remove_file(package.join("b.wav")).unwrap();

        fs::create_dir_all(package.join("kit/.git")).unwrap();
        let error = available(&packs, "fugue.a.b").unwrap_err();
        assert!(error.contains("is a .git directory"), "{error}");
        fs::remove_dir_all(package.join("kit")).unwrap();

        // The package directory itself linked in from elsewhere.
        manifest(&tmp.path().join("elsewhere"), "fugue.c.d", "1.0.0");
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), packs.join("fugue.c.d")).unwrap();
        let error = available(&packs, "fugue.c.d").unwrap_err();
        assert!(error.contains("is a symlink"), "{error}");
    }
}
