//! Hybrid audio asset references: package refs and local paths.
//!
//! Module configs reference audio assets under an `asset` key in one of two
//! authored forms (FUG-130):
//!
//! ```json
//! "asset": "fugue.drums.808@1.2.0:kick/long.wav"
//! ```
//!
//! ```json
//! "asset": { "path": "./loops/melody.wav" }
//! ```
//!
//! The string form is a *package ref* (`id@requirement:file`) when it parses
//! as one, and a plain file path otherwise. Resolution order is: package
//! cache → relative to the invention file → absolute path. Package refs
//! resolve against the installed package cache (`~/.fugue/packs` by default;
//! see [`default_packages_dir`]); the invention loader records the resolved
//! version and integrity hash in `fugue.lock.json` (see
//! `crate::invention::audio_assets`).

use serde::{Deserialize, Serialize};

use semver::VersionReq;

/// An authored audio asset reference, as it appears in module config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum AudioAssetRef {
    /// String form: a package ref (`id@requirement:file`) when it parses as
    /// one (see [`PackageAudioRef::parse`]), otherwise a plain file path.
    Text(String),
    /// Object form: always a local path, relative paths resolving from the
    /// invention file.
    Local { path: String },
}

/// A parsed package asset ref: `fugue.drums.808@1.2.0:kick/long.wav`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageAudioRef {
    /// Reverse-DNS package id (`fugue.drums.808`).
    pub id: String,
    /// Semver requirement text (`1.2.0`, `^1.2`, ...). npm-style: a bare
    /// version means caret semantics, matching package `deps` refs.
    pub requirement: String,
    /// File path inside the installed package, `/`-separated.
    pub path: String,
}

impl PackageAudioRef {
    /// Parse `id@requirement:file`. Returns `None` unless all three parts are
    /// present and well-formed: the id restricted to `[A-Za-z0-9._-]`, the
    /// requirement a valid semver requirement, and the file a clean relative
    /// path. Strings that fail any check are treated as plain file paths by
    /// callers, which keeps paths like `notes@morning:draft.wav` loadable.
    pub fn parse(raw: &str) -> Option<Self> {
        let (id, rest) = raw.split_once('@')?;
        let (requirement, path) = rest.split_once(':')?;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return None;
        }
        if VersionReq::parse(requirement).is_err() {
            return None;
        }
        if !crate::pkg::sample_pack::is_valid_relative_path(path) {
            return None;
        }
        Some(Self {
            id: id.to_string(),
            requirement: requirement.to_string(),
            path: path.to_string(),
        })
    }
}

impl std::fmt::Display for PackageAudioRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.id, self.requirement, self.path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod fs_ops {
    use super::PackageAudioRef;
    use crate::pkg::resolve::select_version;
    use semver::{Version, VersionReq};
    use std::path::{Path, PathBuf};

    /// A package asset ref resolved against the installed package cache.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ResolvedPackageAsset {
        /// Package id.
        pub id: String,
        /// Concrete installed version the requirement resolved to.
        pub version: Version,
        /// Canonical install dir: `packages_dir/<id>/<version>`.
        pub install_dir: PathBuf,
        /// The referenced file inside the install dir.
        pub file: PathBuf,
    }

    /// The default installed-package cache: `$FUGUE_PACKS_DIR` when set,
    /// otherwise `~/.fugue/packs` (the same location `fugue install` stages
    /// packages into).
    pub fn default_packages_dir() -> Result<PathBuf, String> {
        if let Some(dir) = std::env::var_os("FUGUE_PACKS_DIR") {
            return Ok(PathBuf::from(dir));
        }
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".fugue").join("packs"))
            .ok_or_else(|| "Could not determine home directory for package cache".to_string())
    }

    /// Installed versions of `id` under `packages_dir` (directory names that
    /// parse as semver). Missing package dir yields an empty list.
    fn installed_versions(packages_dir: &Path, id: &str) -> Vec<Version> {
        let Ok(entries) = std::fs::read_dir(packages_dir.join(id)) else {
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

    /// Resolve a package asset ref against the installed cache.
    ///
    /// The version is chosen as: the `locked` version when it still satisfies
    /// the requirement and is installed (lockfile pin wins), otherwise the
    /// highest installed version satisfying the requirement. The referenced
    /// file must exist inside the chosen install.
    pub fn resolve_package_asset(
        reference: &PackageAudioRef,
        packages_dir: &Path,
        locked: Option<&str>,
    ) -> Result<ResolvedPackageAsset, String> {
        let requirement = VersionReq::parse(&reference.requirement)
            .map_err(|err| format!("invalid version requirement in '{}': {}", reference, err))?;
        let available = installed_versions(packages_dir, &reference.id);

        let locked_version = locked
            .and_then(|version| Version::parse(version).ok())
            .filter(|version| requirement.matches(version) && available.contains(version));
        let version = match locked_version {
            Some(version) => version,
            None => select_version(&available, &requirement).ok_or_else(|| {
                format!(
                    "no installed version of {} satisfies {} (looked in {}); \
                     run `fugue install {}@{}`",
                    reference.id,
                    reference.requirement,
                    packages_dir.display(),
                    reference.id,
                    reference.requirement,
                )
            })?,
        };

        let install_dir = packages_dir.join(&reference.id).join(version.to_string());
        let file = install_dir.join(&reference.path);
        if !file.is_file() {
            return Err(format!(
                "package {}@{} does not contain '{}' (expected {})",
                reference.id,
                version,
                reference.path,
                file.display()
            ));
        }

        Ok(ResolvedPackageAsset {
            id: reference.id.clone(),
            version,
            install_dir,
            file,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs_ops::{default_packages_dir, resolve_package_asset, ResolvedPackageAsset};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_ref() {
        let parsed = PackageAudioRef::parse("fugue.drums.808@1.2.0:kick/long.wav").unwrap();
        assert_eq!(parsed.id, "fugue.drums.808");
        assert_eq!(parsed.requirement, "1.2.0");
        assert_eq!(parsed.path, "kick/long.wav");
        assert_eq!(parsed.to_string(), "fugue.drums.808@1.2.0:kick/long.wav");
    }

    #[test]
    fn rejects_non_package_strings() {
        // Missing parts.
        assert_eq!(PackageAudioRef::parse("kick/long.wav"), None);
        assert_eq!(PackageAudioRef::parse("fugue.drums.808@1.2.0"), None);
        assert_eq!(PackageAudioRef::parse("@1.2.0:kick.wav"), None);
        assert_eq!(PackageAudioRef::parse("fugue.drums.808@:kick.wav"), None);
        assert_eq!(PackageAudioRef::parse("fugue.drums.808@1.2.0:"), None);
        // URLs: the id would contain '/' (and the "requirement" is not semver).
        assert_eq!(PackageAudioRef::parse("https://x.com/a@b:c.wav"), None);
        // Non-semver requirement stays a plain (loadable) file path.
        assert_eq!(PackageAudioRef::parse("notes@morning:draft.wav"), None);
        // Escaping / absolute file paths inside a package are invalid.
        assert_eq!(PackageAudioRef::parse("fugue.a@1.0:../kick.wav"), None);
        assert_eq!(PackageAudioRef::parse("fugue.a@1.0:/kick.wav"), None);
    }

    #[test]
    fn deserializes_both_authored_forms() {
        let text: AudioAssetRef =
            serde_json::from_value(serde_json::json!("fugue.drums.808@1.2.0:kick.wav")).unwrap();
        assert_eq!(
            text,
            AudioAssetRef::Text("fugue.drums.808@1.2.0:kick.wav".into())
        );

        let local: AudioAssetRef =
            serde_json::from_value(serde_json::json!({ "path": "./loops/melody.wav" })).unwrap();
        assert_eq!(
            local,
            AudioAssetRef::Local {
                path: "./loops/melody.wav".into()
            }
        );
    }
}

/// Test-only helper: point `FUGUE_PACKS_DIR` (the [`default_packages_dir`]
/// override) at a temp cache for the duration of `body`, serialized across
/// the whole test binary because the environment is process-global.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn with_packs_dir(packs: &std::path::Path, body: impl FnOnce()) {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("FUGUE_PACKS_DIR");
    std::env::set_var("FUGUE_PACKS_DIR", packs);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    match previous {
        Some(value) => std::env::set_var("FUGUE_PACKS_DIR", value),
        None => std::env::remove_var("FUGUE_PACKS_DIR"),
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fs_tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn install(packs: &Path, id: &str, version: &str, files: &[&str]) {
        let dir = packs.join(id).join(version);
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"audio").unwrap();
        }
    }

    fn parse(raw: &str) -> PackageAudioRef {
        PackageAudioRef::parse(raw).unwrap()
    }

    #[test]
    fn resolves_highest_satisfying_installed_version() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "fugue.drums.808", "1.2.0", &["kick/long.wav"]);
        install(tmp.path(), "fugue.drums.808", "1.4.0", &["kick/long.wav"]);
        install(tmp.path(), "fugue.drums.808", "2.0.0", &["kick/long.wav"]);

        let resolved = resolve_package_asset(
            &parse("fugue.drums.808@^1.2:kick/long.wav"),
            tmp.path(),
            None,
        )
        .unwrap();
        assert_eq!(resolved.version.to_string(), "1.4.0");
        assert!(resolved
            .file
            .ends_with("fugue.drums.808/1.4.0/kick/long.wav"));
        assert!(resolved.file.is_file());
    }

    #[test]
    fn locked_version_pins_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "fugue.drums.808", "1.2.0", &["kick.wav"]);
        install(tmp.path(), "fugue.drums.808", "1.4.0", &["kick.wav"]);

        let reference = parse("fugue.drums.808@^1.2:kick.wav");
        let resolved = resolve_package_asset(&reference, tmp.path(), Some("1.2.0")).unwrap();
        assert_eq!(resolved.version.to_string(), "1.2.0");

        // A locked version that no longer satisfies the ref (or is not
        // installed) falls back to normal selection.
        let resolved = resolve_package_asset(&reference, tmp.path(), Some("0.9.0")).unwrap();
        assert_eq!(resolved.version.to_string(), "1.4.0");
        let resolved = resolve_package_asset(&reference, tmp.path(), Some("1.3.0")).unwrap();
        assert_eq!(resolved.version.to_string(), "1.4.0");
    }

    #[test]
    fn missing_version_or_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "fugue.drums.808", "1.2.0", &["kick.wav"]);

        let err = resolve_package_asset(&parse("fugue.drums.808@^2.0:kick.wav"), tmp.path(), None)
            .unwrap_err();
        assert!(err.contains("no installed version"));
        assert!(err.contains("fugue install"));

        let err =
            resolve_package_asset(&parse("fugue.drums.808@1.2.0:snare.wav"), tmp.path(), None)
                .unwrap_err();
        assert!(err.contains("does not contain 'snare.wav'"));
    }
}
