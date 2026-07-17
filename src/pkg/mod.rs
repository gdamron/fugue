//! `fugue.pkg.json` manifest schema and validation.
//!
//! See `src/pkg/README.md` for the field-by-field schema and per-kind
//! example manifests, and `src/pkg/SAMPLE_PACK.md` for the `sample-pack`
//! entry (`samples.json`) schema. This module is consumed by the daemon,
//! CLI, and MCP server so they all read the same structs.

pub mod audio_asset;
pub mod lock;
pub mod manifest;
pub mod resolve;
pub mod sample_pack;
pub mod validate;

pub use audio_asset::{AudioAssetRef, PackageAudioRef};
pub use lock::{LockError, LockSource, LockedPackage, Lockfile, LOCKFILE_NAME, LOCKFILE_VERSION};
pub use manifest::{
    Author, Capability, DepRef, EntrySpec, PackageKind, PackageManifest, Requires, Signing, Target,
};
pub use resolve::{
    dependency_edges, resolve_transitive, select_version, PackageProvider, Resolved,
};
pub use sample_pack::{
    parse_str as parse_sample_pack_str, validate as validate_sample_pack, SampleFile,
    SamplePackError, SamplePackManifest, SamplePackValidationError, SampleSlice,
};
pub use validate::{parse_str, validate, ManifestError, ValidationError};

#[cfg(not(target_arch = "wasm32"))]
pub use audio_asset::{default_packages_dir, resolve_package_asset, ResolvedPackageAsset};

#[cfg(not(target_arch = "wasm32"))]
pub use lock::compute_integrity;

#[cfg(not(target_arch = "wasm32"))]
pub use sample_pack::parse_path as parse_sample_pack_path;

#[cfg(not(target_arch = "wasm32"))]
pub use validate::parse_path;
