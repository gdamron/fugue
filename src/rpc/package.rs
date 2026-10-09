//! Package inventory and installation payloads.
//!
//! Listing is local: built-in module types plus the packages installed in the
//! daemon's package cache. It never contacts a registry. See
//! [`PackageList::local`] for the bounds every page obeys.

use crate::pkg::content::{ContentError, ContentRef};
use crate::pkg::PackageKind;
use crate::ModuleRegistry;
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
mod local;

/// Wire version of package list, detail and install payloads.
pub const PACKAGE_SCHEMA_VERSION: u32 = 1;
/// Largest serialized page of [`PackageList::packages`], in bytes.
pub const MAX_PACKAGE_LIST_BYTES: usize = 64 * 1024;
/// Longest [`PackageInfo::summary`], in UTF-8 bytes.
pub const MAX_PACKAGE_SUMMARY_BYTES: usize = 512;
/// Most diagnostics returned with one page.
pub const MAX_PACKAGE_DIAGNOSTICS: usize = 20;

/// A bounded request for one page of the local package inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PackageListQuery {
    /// Must be 1.
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    /// Only packages of this manifest kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PackageKind>,
    /// Only packages with this provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PackageSource>,
    /// Only the package with this exact ID (every installed version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Entries per page, 1–100.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Opaque cursor from a previous page with the same filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Include dependencies and module types. Off by default to keep pages small.
    #[serde(default)]
    pub detail: bool,
}

fn schema_version() -> u32 {
    PACKAGE_SCHEMA_VERSION
}

fn default_limit() -> usize {
    20
}

impl Default for PackageListQuery {
    fn default() -> Self {
        Self {
            schema_version: PACKAGE_SCHEMA_VERSION,
            kind: None,
            source: None,
            id: None,
            limit: default_limit(),
            cursor: None,
            detail: false,
        }
    }
}

/// One page of the package inventory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageList {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub packages: Vec<PackageInfo>,
    /// Present when more entries match; pass it back as `cursor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Installed directories that could not be listed, at most
    /// [`MAX_PACKAGE_DIAGNOSTICS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ContentError>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diagnostics_truncated: bool,
}

impl PackageList {
    /// The built-in module registry as one package entry, with full detail.
    pub fn built_in(registry: &ModuleRegistry) -> Self {
        Self {
            schema_version: PACKAGE_SCHEMA_VERSION,
            packages: vec![PackageInfo::built_in(registry)],
            next_cursor: None,
            diagnostics: Vec::new(),
            diagnostics_truncated: false,
        }
    }
}

/// One package version known to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageInfo {
    /// Manifest ID; `builtin` for the compiled-in module types.
    #[serde(alias = "name")]
    pub id: String,
    /// Exact installed SemVer.
    pub version: String,
    pub kind: PackageKind,
    pub source: PackageSource,
    /// Manifest description, truncated to [`MAX_PACKAGE_SUMMARY_BYTES`].
    #[serde(default)]
    pub summary: String,
    /// Exact reference for `describe_development`, `describe_example`, and
    /// `developments[].ref`. Present for development and invention packages.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub content_ref: Option<ContentRef>,
    /// Declared `id@requirement` dependencies (detail only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    /// Module types the package provides (detail only; built-in today).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_types: Vec<String>,
}

impl PackageInfo {
    /// The compiled-in module registry, with its sorted type names.
    pub fn built_in(registry: &ModuleRegistry) -> Self {
        let mut module_types: Vec<String> = registry.types().map(str::to_string).collect();
        module_types.sort();
        Self {
            id: "builtin".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            kind: PackageKind::Module,
            source: PackageSource::BuiltIn,
            summary: "Module types compiled into this Fugue build.".to_string(),
            content_ref: None,
            dependencies: Vec::new(),
            module_types,
        }
    }

    /// Drop the detail-only fields.
    pub fn compact(mut self) -> Self {
        self.dependencies.clear();
        self.module_types.clear();
        self
    }
}

/// Where a listed package came from. Provenance, not identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PackageSource {
    BuiltIn,
    /// Reserved; no listing reports it.
    External,
    /// Staged from content shipped with the release.
    Bundled,
    /// Installed into the package cache by an install command.
    Installed,
}

/// Install one package (and its dependencies) into the daemon's package cache.
///
/// `package` takes the same forms as `fugue install`:
/// `local:<absolute directory>`, `github:<owner>/<repo>[@<ref>]`, or a registry
/// `<id>@<version>`. A registry ID may instead pass its version in `version`;
/// `version` is refused for local and GitHub sources, whose manifest decides it.
/// The command is flattened into the request envelope, whose `schema_version`
/// versions it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PackageInstallRequest {
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// The committed result of [`PackageInstallRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageInstallReport {
    pub schema_version: u32,
    /// The installed root package, as `list_packages` reports it.
    pub package: PackageInfo,
    /// True when this exact version was already installed with identical
    /// bytes; nothing was copied.
    pub already_installed: bool,
    /// The source the package was installed from, in request syntax.
    pub installed_from: String,
    /// Dependencies installed or confirmed alongside it, as `id@version`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    /// Content catalog generation that already includes the package, for
    /// development and invention packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
}
