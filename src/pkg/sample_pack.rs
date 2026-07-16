//! `sample-pack` entry (`samples.json`) types.
//!
//! A `sample-pack` package's `fugue.pkg.json` names its entry file via
//! `entry.samples` (conventionally `samples.json`). That entry file is the
//! kind-specific manifest defined here: licensing, attribution, declared
//! sample rates, free-form tags, and an explicit file listing with
//! per-file overrides and optional slice points (consumed by
//! `sample_slicer`). See `src/pkg/SAMPLE_PACK.md` for the field-by-field
//! schema.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::manifest::Author;

/// A sample-pack entry manifest (`samples.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SamplePackManifest {
    /// Pack-wide SPDX license identifier. Individual files may override.
    pub license: String,

    /// Creators of the audio content. May differ from the package
    /// manifest's `authors` (the people who assembled the pack).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribution: Vec<Author>,

    /// Sample rates present in the pack, in Hz. Every file resolves to
    /// exactly one of these: its own `sample_rate` override, or — when the
    /// pack declares a single rate — the pack-wide rate.
    pub sample_rate: Vec<u32>,

    /// Free-form descriptive tags (`genre`, `instrument`, `bpm`, `key`,
    /// ...). Values are strings even when numeric (`"bpm": "120"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,

    /// Explicit listing of every audio file in the pack.
    pub files: Vec<SampleFile>,
}

/// One audio file in a sample pack, with optional per-file overrides of
/// the pack-wide defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SampleFile {
    /// Path relative to the package root (e.g. `samples/kick.wav`).
    pub path: String,

    /// SPDX license override for this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Attribution override for this file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribution: Vec<Author>,

    /// This file's sample rate in Hz. Required when the pack declares
    /// more than one rate; must be one of the declared rates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,

    /// Free-form tags for this file, merged over the pack-wide tags.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,

    /// Optional slice points into this file, in frames. Slices may
    /// overlap (layered or round-robin chops are legitimate).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slices: Vec<SampleSlice>,
}

/// A named region of a sample file, addressed in frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SampleSlice {
    /// First frame of the slice (inclusive).
    pub start_frames: u64,

    /// End frame of the slice (exclusive). Must be greater than
    /// `start_frames`.
    pub end_frames: u64,

    /// Optional name for addressing the slice (`"kick"`, `"snare"`).
    /// Unnamed slices are addressed by index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl SamplePackManifest {
    /// The license governing `file`: its override, or the pack license.
    pub fn license_for<'a>(&'a self, file: &'a SampleFile) -> &'a str {
        file.license.as_deref().unwrap_or(&self.license)
    }

    /// The attribution for `file`: its override, or the pack attribution.
    pub fn attribution_for<'a>(&'a self, file: &'a SampleFile) -> &'a [Author] {
        if file.attribution.is_empty() {
            &self.attribution
        } else {
            &file.attribution
        }
    }

    /// The sample rate of `file`: its override, or the pack-wide rate
    /// when exactly one is declared. `None` only on manifests that fail
    /// [`validate`].
    pub fn sample_rate_for(&self, file: &SampleFile) -> Option<u32> {
        file.sample_rate.or(match self.sample_rate.as_slice() {
            [rate] => Some(*rate),
            _ => None,
        })
    }
}

/// Reasons a sample-pack entry can fail to load.
#[derive(Debug)]
pub enum SamplePackError {
    /// Underlying JSON deserialization failed.
    Json(serde_json::Error),
    /// File I/O failed (only produced by [`parse_path`]).
    Io(std::io::Error),
    /// Entry deserialized but failed validation.
    Invalid(SamplePackValidationError),
}

impl fmt::Display for SamplePackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SamplePackError::Json(e) => write!(f, "sample-pack entry JSON parse error: {e}"),
            SamplePackError::Io(e) => write!(f, "sample-pack entry I/O error: {e}"),
            SamplePackError::Invalid(e) => write!(f, "sample-pack entry invalid: {e}"),
        }
    }
}

impl std::error::Error for SamplePackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SamplePackError::Json(e) => Some(e),
            SamplePackError::Io(e) => Some(e),
            SamplePackError::Invalid(_) => None,
        }
    }
}

impl From<serde_json::Error> for SamplePackError {
    fn from(value: serde_json::Error) -> Self {
        SamplePackError::Json(value)
    }
}

impl From<std::io::Error> for SamplePackError {
    fn from(value: std::io::Error) -> Self {
        SamplePackError::Io(value)
    }
}

impl From<SamplePackValidationError> for SamplePackError {
    fn from(value: SamplePackValidationError) -> Self {
        SamplePackError::Invalid(value)
    }
}

/// Specific sample-pack validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamplePackValidationError {
    EmptyLicense,
    EmptyFileLicense {
        path: String,
    },
    EmptyAttributionName,
    EmptySampleRates,
    InvalidSampleRate(u32),
    NoFiles,
    InvalidFilePath(String),
    DuplicateFilePath(String),
    /// A file's `sample_rate` override is not in the declared list.
    UndeclaredSampleRate {
        path: String,
        rate: u32,
    },
    /// The pack declares multiple rates and this file picks none.
    AmbiguousSampleRate {
        path: String,
    },
    /// A declared rate that no file resolves to.
    UnusedSampleRate(u32),
    /// `end_frames` is not greater than `start_frames`.
    InvalidSliceRange {
        path: String,
        index: usize,
    },
    EmptySliceName {
        path: String,
        index: usize,
    },
    DuplicateSliceName {
        path: String,
        name: String,
    },
}

impl fmt::Display for SamplePackValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SamplePackValidationError::EmptyLicense => write!(f, "license must not be empty"),
            SamplePackValidationError::EmptyFileLicense { path } => {
                write!(f, "`{path}`: license override must not be empty")
            }
            SamplePackValidationError::EmptyAttributionName => {
                write!(f, "attribution name must not be empty")
            }
            SamplePackValidationError::EmptySampleRates => {
                write!(f, "sample_rate must declare at least one rate")
            }
            SamplePackValidationError::InvalidSampleRate(rate) => {
                write!(f, "invalid sample rate `{rate}`: must be greater than zero")
            }
            SamplePackValidationError::NoFiles => {
                write!(f, "files must list at least one sample")
            }
            SamplePackValidationError::InvalidFilePath(path) => {
                write!(
                    f,
                    "invalid file path `{path}`: expected a relative path inside the package"
                )
            }
            SamplePackValidationError::DuplicateFilePath(path) => {
                write!(f, "duplicate file path `{path}`")
            }
            SamplePackValidationError::UndeclaredSampleRate { path, rate } => {
                write!(
                    f,
                    "`{path}`: sample rate {rate} is not declared in the pack's sample_rate list"
                )
            }
            SamplePackValidationError::AmbiguousSampleRate { path } => {
                write!(
                    f,
                    "`{path}`: pack declares multiple sample rates, so the file must declare its own"
                )
            }
            SamplePackValidationError::UnusedSampleRate(rate) => {
                write!(f, "declared sample rate {rate} is not used by any file")
            }
            SamplePackValidationError::InvalidSliceRange { path, index } => {
                write!(
                    f,
                    "`{path}` slice {index}: end_frames must be greater than start_frames"
                )
            }
            SamplePackValidationError::EmptySliceName { path, index } => {
                write!(f, "`{path}` slice {index}: name must not be empty")
            }
            SamplePackValidationError::DuplicateSliceName { path, name } => {
                write!(f, "`{path}`: duplicate slice name `{name}`")
            }
        }
    }
}

/// Validate an already-deserialized sample-pack entry. Returns the first
/// failure found.
pub fn validate(manifest: &SamplePackManifest) -> Result<(), SamplePackValidationError> {
    if manifest.license.trim().is_empty() {
        return Err(SamplePackValidationError::EmptyLicense);
    }
    validate_attribution(&manifest.attribution)?;
    if manifest.sample_rate.is_empty() {
        return Err(SamplePackValidationError::EmptySampleRates);
    }
    for rate in &manifest.sample_rate {
        if *rate == 0 {
            return Err(SamplePackValidationError::InvalidSampleRate(0));
        }
    }
    if manifest.files.is_empty() {
        return Err(SamplePackValidationError::NoFiles);
    }

    let mut paths = BTreeSet::new();
    let mut used_rates = BTreeSet::new();
    for file in &manifest.files {
        if !is_valid_relative_path(&file.path) {
            return Err(SamplePackValidationError::InvalidFilePath(
                file.path.clone(),
            ));
        }
        if !paths.insert(file.path.as_str()) {
            return Err(SamplePackValidationError::DuplicateFilePath(
                file.path.clone(),
            ));
        }
        if let Some(license) = &file.license {
            if license.trim().is_empty() {
                return Err(SamplePackValidationError::EmptyFileLicense {
                    path: file.path.clone(),
                });
            }
        }
        validate_attribution(&file.attribution)?;
        if let Some(rate) = file.sample_rate {
            if !manifest.sample_rate.contains(&rate) {
                return Err(SamplePackValidationError::UndeclaredSampleRate {
                    path: file.path.clone(),
                    rate,
                });
            }
        }
        match manifest.sample_rate_for(file) {
            Some(rate) => {
                used_rates.insert(rate);
            }
            None => {
                return Err(SamplePackValidationError::AmbiguousSampleRate {
                    path: file.path.clone(),
                })
            }
        }
        validate_slices(file)?;
    }

    for rate in &manifest.sample_rate {
        if !used_rates.contains(rate) {
            return Err(SamplePackValidationError::UnusedSampleRate(*rate));
        }
    }
    Ok(())
}

fn validate_attribution(attribution: &[Author]) -> Result<(), SamplePackValidationError> {
    for author in attribution {
        if author.name.trim().is_empty() {
            return Err(SamplePackValidationError::EmptyAttributionName);
        }
    }
    Ok(())
}

fn validate_slices(file: &SampleFile) -> Result<(), SamplePackValidationError> {
    let mut names = BTreeSet::new();
    for (index, slice) in file.slices.iter().enumerate() {
        if slice.end_frames <= slice.start_frames {
            return Err(SamplePackValidationError::InvalidSliceRange {
                path: file.path.clone(),
                index,
            });
        }
        if let Some(name) = &slice.name {
            if name.trim().is_empty() {
                return Err(SamplePackValidationError::EmptySliceName {
                    path: file.path.clone(),
                    index,
                });
            }
            if !names.insert(name.as_str()) {
                return Err(SamplePackValidationError::DuplicateSliceName {
                    path: file.path.clone(),
                    name: name.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Parse and validate a sample-pack entry from a JSON string.
pub fn parse_str(input: &str) -> Result<SamplePackManifest, SamplePackError> {
    let manifest: SamplePackManifest = serde_json::from_str(input)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Parse and validate a sample-pack entry from a path on disk.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_path(
    path: impl AsRef<std::path::Path>,
) -> Result<SamplePackManifest, SamplePackError> {
    let bytes = std::fs::read(path.as_ref())?;
    let manifest: SamplePackManifest = serde_json::from_slice(&bytes)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Relative-path check: non-empty, `/`-separated, no absolute prefix, no
/// `.`/`..` segments, no backslashes (portability).
fn is_valid_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    path.split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}
