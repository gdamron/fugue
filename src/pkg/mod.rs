//! `fugue.pkg.json` manifest schema and validation.
//!
//! See `src/pkg/README.md` for the field-by-field schema and per-kind
//! example manifests. This module is consumed by the daemon, CLI, and
//! MCP server so they all read the same struct.

pub mod manifest;
pub mod validate;

pub use manifest::{
    Author, Capability, DepRef, EntrySpec, PackageKind, PackageManifest, Requires, Signing, Target,
};
pub use validate::{parse_str, validate, ManifestError, ValidationError};

#[cfg(not(target_arch = "wasm32"))]
pub use validate::parse_path;
