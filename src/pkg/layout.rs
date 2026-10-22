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
//! The rules, which every resolver path shares:
//!
//! - A source checkout offers exactly its manifest's version. Requirements
//!   and lockfile pins match it as they would an installed version, so an
//!   exact pin (`=2026.10.2`) needs that exact manifest version.
//! - Its manifest must parse and validate, and its `id` must equal the
//!   directory name. Otherwise the package is refused, never skipped.
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

/// The versions of `id` under `packages_dir`, in whichever layout holds it.
/// A missing package directory offers no versions.
pub(crate) fn available(packages_dir: &Path, id: &str) -> Result<Available, String> {
    let package_dir = packages_dir.join(id);
    let installed = installed_versions(&package_dir);
    let manifest_path = package_dir.join("fugue.pkg.json");
    if !manifest_path.is_file() {
        return Ok(Available::Installed(installed));
    }
    if !installed.is_empty() {
        return Err(format!(
            "{} holds both installed versions ({id}/<version>/) and a source checkout \
             ({id}/fugue.pkg.json); remove one so the package resolves unambiguously",
            package_dir.display()
        ));
    }
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
}
