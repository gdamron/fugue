//! `fugue.lock.json` — the reproducible-install lockfile.
//!
//! The lockfile pins every installed (β) package to a concrete resolved
//! version, the source it was fetched from, and an integrity hash over the
//! installed directory contents. `fugue install` writes it (see the
//! `fugue-cli` `lockwrite` module) and invention loaders [`verify`] it so a
//! tampered or missing package is caught before audio starts.
//!
//! [`verify`]: Lockfile::verify
//!
//! The schema is intentionally small and stable; see `src/pkg/README.md` for
//! the surrounding package model. Version 1 lockfiles (pre-integrity) are read
//! and upgraded in memory so older projects keep loading.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Current lockfile schema version.
pub const LOCKFILE_VERSION: u32 = 2;

/// Conventional lockfile filename (next to an invention or in the data dir).
pub const LOCKFILE_NAME: &str = "fugue.lock.json";

/// Where a locked package was resolved from.
///
/// Serializes as an externally-tagged object, e.g.
/// `{ "registry": { "id": "...", "version": "..." } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockSource {
    /// Resolved through the (stubbed) hosted registry index.
    Registry { id: String, version: String },
    /// Cloned from a git repository, optionally at a ref.
    Git {
        repository: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    /// Installed from a local directory.
    Local { path: String },
}

impl LockSource {
    /// Parse a legacy v1 `source` string (`local:…`, `github:repo[@ref]`, or
    /// `id@version`) into a structured source. Used only for v1 upgrade.
    pub fn parse_legacy(raw: &str) -> Self {
        if let Some(path) = raw.strip_prefix("local:") {
            return LockSource::Local {
                path: path.to_string(),
            };
        }
        if let Some(rest) = raw.strip_prefix("github:") {
            return match rest.split_once('@') {
                Some((repository, reference)) => LockSource::Git {
                    repository: repository.to_string(),
                    reference: Some(reference.to_string()),
                },
                None => LockSource::Git {
                    repository: rest.to_string(),
                    reference: None,
                },
            };
        }
        match raw.split_once('@') {
            Some((id, version)) if !id.is_empty() && !version.is_empty() => LockSource::Registry {
                id: id.to_string(),
                version: version.to_string(),
            },
            _ => LockSource::Local {
                path: raw.to_string(),
            },
        }
    }
}

/// One resolved package entry. The package id is the map key in
/// [`Lockfile::packages`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    /// Resolved semver of the package.
    pub version: String,
    /// Package kind (`module`, `development`, …) as a string.
    pub kind: String,
    /// Where the package came from.
    pub source: LockSource,
    /// `sha256:<hex>` over the installed directory contents. Empty only for
    /// entries upgraded from a v1 lockfile that predates integrity hashing.
    pub integrity: String,
    /// Absolute install path (informational; [`Lockfile::verify`] derives the
    /// canonical path from the packages dir instead).
    pub path: PathBuf,
    /// Resolved dependency edges as `id@version` strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// The parsed `fugue.lock.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    /// Schema version. Always [`LOCKFILE_VERSION`] after construction/upgrade.
    pub version: u32,
    /// Explicitly-installed package ids (graph roots).
    #[serde(default)]
    pub roots: Vec<String>,
    /// All locked packages keyed by id (sorted on write via `BTreeMap`).
    #[serde(default)]
    pub packages: BTreeMap<String, LockedPackage>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            roots: Vec::new(),
            packages: BTreeMap::new(),
        }
    }
}

impl Lockfile {
    /// A new, empty v2 lockfile.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a lockfile from raw bytes, upgrading v1 documents in memory.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Box<dyn Error>> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        match version {
            2 => Ok(serde_json::from_value(value)?),
            1 => Self::from_v1(&value),
            other => Err(format!("unsupported lockfile version {other}").into()),
        }
    }

    /// Upgrade a v1 document (`{version,kind,source:string,path}` entries, no
    /// integrity) into the in-memory v2 shape. Integrity is left empty so a
    /// frozen load reports it as unverifiable until `fugue install` refreshes.
    fn from_v1(value: &serde_json::Value) -> Result<Self, Box<dyn Error>> {
        let mut lock = Lockfile::new();
        if let Some(packages) = value.get("packages").and_then(|p| p.as_object()) {
            for (id, entry) in packages {
                let version = entry
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let kind = entry
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let source = entry
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(LockSource::parse_legacy)
                    .unwrap_or(LockSource::Local {
                        path: String::new(),
                    });
                let path = entry
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_default();
                lock.packages.insert(
                    id.clone(),
                    LockedPackage {
                        version,
                        kind,
                        source,
                        integrity: String::new(),
                        path,
                        dependencies: Vec::new(),
                    },
                );
            }
        }
        Ok(lock)
    }

    /// Serialize to deterministic pretty JSON with a trailing newline. Package
    /// keys are sorted (`BTreeMap`) and roots are sorted/deduped.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut out = self.clone();
        out.version = LOCKFILE_VERSION;
        out.roots.sort();
        out.roots.dedup();
        let mut bytes = serde_json::to_vec_pretty(&out)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Insert or replace a package entry.
    pub fn upsert(&mut self, id: impl Into<String>, package: LockedPackage) {
        self.packages.insert(id.into(), package);
    }

    /// Record an explicitly-installed package id as a graph root.
    pub fn add_root(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !self.roots.contains(&id) {
            self.roots.push(id);
        }
    }
}

/// Aggregated lockfile validation failure, naming each offending package.
#[derive(Debug)]
pub struct LockError {
    /// One human-readable problem per failing package.
    pub problems: Vec<String>,
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "lockfile validation failed:\n  - {}",
            self.problems.join("\n  - ")
        )
    }
}

impl Error for LockError {}

#[cfg(not(target_arch = "wasm32"))]
mod fs_ops {
    use super::{LockError, Lockfile, LOCKFILE_NAME};
    use sha2::{Digest, Sha256};
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};

    impl Lockfile {
        /// Read and parse (and upgrade) a lockfile from disk.
        pub fn read(path: &Path) -> Result<Self, Box<dyn Error>> {
            Self::from_slice(&fs::read(path)?)
        }

        /// Look for a `fugue.lock.json` next to an invention file.
        pub fn find_beside(invention_path: &Path) -> Option<PathBuf> {
            let candidate = invention_path.parent()?.join(LOCKFILE_NAME);
            candidate.is_file().then_some(candidate)
        }

        /// Verify every locked package is present under `packages_dir` and its
        /// contents still hash to the recorded integrity. The canonical install
        /// location `packages_dir/<id>/<version>` is used, so the lockfile is
        /// portable across machines.
        pub fn verify(&self, packages_dir: &Path) -> Result<(), LockError> {
            let mut problems = Vec::new();
            for (id, package) in &self.packages {
                let dir = packages_dir.join(id).join(&package.version);
                if !dir.is_dir() {
                    problems.push(format!(
                        "{id}@{} is not installed (expected {})",
                        package.version,
                        dir.display()
                    ));
                    continue;
                }
                if package.integrity.is_empty() {
                    problems.push(format!(
                        "{id}@{} has no integrity hash; run `fugue install` to refresh the lockfile",
                        package.version
                    ));
                    continue;
                }
                match compute_integrity(&dir) {
                    Ok(actual) if actual == package.integrity => {}
                    Ok(actual) => problems.push(format!(
                        "{id}@{} integrity mismatch (expected {}, got {actual})",
                        package.version, package.integrity
                    )),
                    Err(err) => problems.push(format!(
                        "{id}@{}: failed to hash installed contents: {err}",
                        package.version
                    )),
                }
            }
            if problems.is_empty() {
                Ok(())
            } else {
                Err(LockError { problems })
            }
        }
    }

    /// Compute `sha256:<hex>` over a directory's contents.
    ///
    /// Files are visited in sorted relative-path order; each contributes its
    /// normalized relative path and raw bytes (both length-prefixed) to the
    /// digest, so the result is stable across platforms and filesystem walk
    /// order. `.git` is skipped; symlinks are ignored (installs never copy
    /// them).
    pub fn compute_integrity(dir: &Path) -> Result<String, Box<dyn Error>> {
        let mut files = Vec::new();
        collect_files(dir, dir, &mut files)?;
        files.sort();
        let mut hasher = Sha256::new();
        for rel in &files {
            let bytes = fs::read(dir.join(rel))?;
            hasher.update((rel.len() as u64).to_le_bytes());
            hasher.update(rel.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }

    /// Collect normalized (`/`-separated) relative file paths under `root`.
    fn collect_files(root: &Path, current: &Path, out: &mut Vec<String>) -> Result<(), Box<dyn Error>> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if entry.file_name() == ".git" {
                    continue;
                }
                collect_files(root, &entry.path(), out)?;
            } else if file_type.is_file() {
                let rel = entry.path().strip_prefix(root)?.to_owned();
                let normalized = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(normalized);
            }
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs_ops::compute_integrity;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Lockfile {
        let mut lock = Lockfile::new();
        lock.add_root("fugue.demo.reverb");
        lock.upsert(
            "fugue.demo.reverb",
            LockedPackage {
                version: "1.2.3".into(),
                kind: "module".into(),
                source: LockSource::Registry {
                    id: "fugue.demo.reverb".into(),
                    version: "1.2.3".into(),
                },
                integrity: "sha256:abc".into(),
                path: PathBuf::from("/packs/fugue.demo.reverb/1.2.3"),
                dependencies: vec!["fugue.demo.util@0.4.0".into()],
            },
        );
        lock
    }

    #[test]
    fn round_trips_through_bytes() {
        let lock = sample();
        let bytes = lock.to_bytes().unwrap();
        let parsed = Lockfile::from_slice(&bytes).unwrap();
        assert_eq!(lock, parsed);
    }

    #[test]
    fn serializes_deterministically_with_trailing_newline() {
        let bytes = sample().to_bytes().unwrap();
        assert_eq!(*bytes.last().unwrap(), b'\n');
        // Stable across repeated serialization.
        assert_eq!(bytes, sample().to_bytes().unwrap());
    }

    #[test]
    fn upgrades_v1_lockfile() {
        let v1 = br#"{
            "version": 1,
            "packages": {
                "fugue.demo.reverb": {
                    "version": "1.2.3",
                    "kind": "module",
                    "source": "github:ilusiv/demo@v1.2.3",
                    "path": "/packs/fugue.demo.reverb/1.2.3"
                }
            }
        }"#;
        let lock = Lockfile::from_slice(v1).unwrap();
        assert_eq!(lock.version, LOCKFILE_VERSION);
        let entry = &lock.packages["fugue.demo.reverb"];
        assert_eq!(entry.version, "1.2.3");
        assert_eq!(entry.integrity, "");
        assert_eq!(
            entry.source,
            LockSource::Git {
                repository: "ilusiv/demo".into(),
                reference: Some("v1.2.3".into()),
            }
        );
    }

    #[test]
    fn parses_legacy_sources() {
        assert_eq!(
            LockSource::parse_legacy("local:/tmp/pkg"),
            LockSource::Local {
                path: "/tmp/pkg".into()
            }
        );
        assert_eq!(
            LockSource::parse_legacy("fugue.demo.util@0.4.0"),
            LockSource::Registry {
                id: "fugue.demo.util".into(),
                version: "0.4.0".into()
            }
        );
        assert_eq!(
            LockSource::parse_legacy("github:ilusiv/demo"),
            LockSource::Git {
                repository: "ilusiv/demo".into(),
                reference: None
            }
        );
    }

    #[test]
    fn rejects_unknown_version() {
        let err = Lockfile::from_slice(br#"{"version": 99}"#).unwrap_err();
        assert!(err.to_string().contains("unsupported lockfile version"));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fs_tests {
    use super::*;
    use std::fs;

    fn write_pkg(root: &std::path::Path, id: &str, version: &str, body: &str) -> std::path::PathBuf {
        let dir = root.join(id).join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("fugue.pkg.json"), body).unwrap();
        dir
    }

    #[test]
    fn integrity_is_stable_and_content_sensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_pkg(tmp.path(), "fugue.demo.x", "1.0.0", "alpha");
        let first = compute_integrity(&dir).unwrap();
        let second = compute_integrity(&dir).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));

        fs::write(dir.join("fugue.pkg.json"), "beta").unwrap();
        assert_ne!(first, compute_integrity(&dir).unwrap());
    }

    #[test]
    fn verify_passes_then_fails_after_tamper() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_pkg(tmp.path(), "fugue.demo.x", "1.0.0", "alpha");
        let integrity = compute_integrity(&dir).unwrap();

        let mut lock = Lockfile::new();
        lock.upsert(
            "fugue.demo.x",
            LockedPackage {
                version: "1.0.0".into(),
                kind: "module".into(),
                source: LockSource::Local {
                    path: dir.display().to_string(),
                },
                integrity,
                path: dir.clone(),
                dependencies: Vec::new(),
            },
        );
        assert!(lock.verify(tmp.path()).is_ok());

        fs::write(dir.join("fugue.pkg.json"), "tampered").unwrap();
        let err = lock.verify(tmp.path()).unwrap_err();
        assert_eq!(err.problems.len(), 1);
        assert!(err.problems[0].contains("fugue.demo.x"));
        assert!(err.problems[0].contains("integrity mismatch"));
    }

    #[test]
    fn verify_reports_missing_install() {
        let tmp = tempfile::tempdir().unwrap();
        let mut lock = Lockfile::new();
        lock.upsert(
            "fugue.demo.missing",
            LockedPackage {
                version: "2.0.0".into(),
                kind: "module".into(),
                source: LockSource::Local { path: String::new() },
                integrity: "sha256:deadbeef".into(),
                path: PathBuf::new(),
                dependencies: Vec::new(),
            },
        );
        let err = lock.verify(tmp.path()).unwrap_err();
        assert!(err.problems[0].contains("is not installed"));
    }
}
