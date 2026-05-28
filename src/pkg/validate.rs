//! Manifest validation and parse helpers.
//!
//! [`parse_str`] / [`parse_path`] are the entry points most callers want:
//! they deserialize a [`PackageManifest`] and run [`validate`] in one shot.

use std::fmt;

use super::manifest::{Capability, DepRef, PackageManifest};

/// Reasons a manifest can fail to load.
#[derive(Debug)]
pub enum ManifestError {
    /// Underlying JSON deserialization failed.
    Json(serde_json::Error),
    /// File I/O failed (only produced by [`parse_path`]).
    Io(std::io::Error),
    /// Manifest deserialized but failed validation.
    Invalid(ValidationError),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Json(e) => write!(f, "manifest JSON parse error: {e}"),
            ManifestError::Io(e) => write!(f, "manifest I/O error: {e}"),
            ManifestError::Invalid(e) => write!(f, "manifest invalid: {e}"),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ManifestError::Json(e) => Some(e),
            ManifestError::Io(e) => Some(e),
            ManifestError::Invalid(_) => None,
        }
    }
}

impl From<serde_json::Error> for ManifestError {
    fn from(value: serde_json::Error) -> Self {
        ManifestError::Json(value)
    }
}

impl From<std::io::Error> for ManifestError {
    fn from(value: std::io::Error) -> Self {
        ManifestError::Io(value)
    }
}

impl From<ValidationError> for ManifestError {
    fn from(value: ValidationError) -> Self {
        ManifestError::Invalid(value)
    }
}

/// Specific validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    InvalidId(String),
    InvalidVersion(String),
    EmptyLicense,
    EmptyTargets,
    EmptyAuthors,
    EmptyAuthorName,
    /// Manifest's `kind` does not match the variant shape of `entry`.
    KindEntryMismatch {
        kind: &'static str,
        entry_kind: &'static str,
    },
    UnknownCapability(String),
    InvalidDepRef(String),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::InvalidId(id) => {
                write!(f, "invalid id `{id}`: expected reverse-dns like `fugue.ns.name` (lowercase, dot-separated, ≥3 segments)")
            }
            ValidationError::InvalidVersion(v) => {
                write!(f, "invalid version `{v}`: expected semver (MAJOR.MINOR.PATCH)")
            }
            ValidationError::EmptyLicense => write!(f, "license must not be empty"),
            ValidationError::EmptyTargets => write!(f, "targets must list at least one surface"),
            ValidationError::EmptyAuthors => write!(f, "authors must not be empty"),
            ValidationError::EmptyAuthorName => write!(f, "author name must not be empty"),
            ValidationError::KindEntryMismatch { kind, entry_kind } => write!(
                f,
                "kind `{kind}` does not match entry shape (entry describes a `{entry_kind}`)"
            ),
            ValidationError::UnknownCapability(c) => {
                write!(f, "unknown or malformed capability `{c}`")
            }
            ValidationError::InvalidDepRef(d) => {
                write!(f, "invalid dep ref `{d}`: expected `id@requirement`")
            }
        }
    }
}

/// Validate an already-deserialized manifest. Returns the first failure
/// found; callers that want a list of all failures should run their own
/// checks.
pub fn validate(manifest: &PackageManifest) -> Result<(), ValidationError> {
    if !is_valid_id(&manifest.id) {
        return Err(ValidationError::InvalidId(manifest.id.clone()));
    }
    if !is_valid_semver(&manifest.version) {
        return Err(ValidationError::InvalidVersion(manifest.version.clone()));
    }
    if manifest.license.trim().is_empty() {
        return Err(ValidationError::EmptyLicense);
    }
    if manifest.targets.is_empty() {
        return Err(ValidationError::EmptyTargets);
    }
    if manifest.authors.is_empty() {
        return Err(ValidationError::EmptyAuthors);
    }
    for author in &manifest.authors {
        if author.name.trim().is_empty() {
            return Err(ValidationError::EmptyAuthorName);
        }
    }
    let entry_kind = manifest.entry.kind();
    if entry_kind != manifest.kind {
        return Err(ValidationError::KindEntryMismatch {
            kind: manifest.kind.as_str(),
            entry_kind: entry_kind.as_str(),
        });
    }
    for cap in &manifest.requires.capabilities {
        if Capability::parse(cap).is_none() {
            return Err(ValidationError::UnknownCapability(cap.clone()));
        }
    }
    for dep in &manifest.deps {
        if DepRef::parse(dep).is_none() {
            return Err(ValidationError::InvalidDepRef(dep.clone()));
        }
    }
    Ok(())
}

/// Parse and validate a manifest from a JSON string.
pub fn parse_str(input: &str) -> Result<PackageManifest, ManifestError> {
    let manifest: PackageManifest = serde_json::from_str(input)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Parse and validate a manifest from a path on disk.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_path(path: impl AsRef<std::path::Path>) -> Result<PackageManifest, ManifestError> {
    let bytes = std::fs::read(path.as_ref())?;
    let manifest: PackageManifest = serde_json::from_slice(&bytes)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Reverse-DNS id check: lowercase ASCII, ≥3 dot-segments, each segment
/// non-empty and made of `[a-z0-9_-]`, leading char alphanumeric.
fn is_valid_id(id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let segments: Vec<&str> = id.split('.').collect();
    if segments.len() < 3 {
        return false;
    }
    for seg in segments {
        if seg.is_empty() {
            return false;
        }
        let mut chars = seg.chars();
        let first = chars.next().unwrap();
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return false;
        }
        for c in std::iter::once(first).chain(chars) {
            let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
            if !ok {
                return false;
            }
        }
    }
    true
}

/// Semver shape check: `MAJOR.MINOR.PATCH` with optional `-pre` and
/// `+build` segments. Each numeric segment must be a non-negative integer
/// with no leading zeros (except `0`).
fn is_valid_semver(v: &str) -> bool {
    let (core, _) = v.split_once('+').unwrap_or((v, ""));
    let (core, pre) = core.split_once('-').unwrap_or((core, ""));
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    for p in &parts {
        if !is_numeric_segment(p) {
            return false;
        }
    }
    if !pre.is_empty() {
        for ident in pre.split('.') {
            if ident.is_empty() {
                return false;
            }
            if !ident
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return false;
            }
        }
    }
    true
}

fn is_numeric_segment(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.len() > 1 && s.starts_with('0') {
        return false;
    }
    s.chars().all(|c| c.is_ascii_digit())
}
