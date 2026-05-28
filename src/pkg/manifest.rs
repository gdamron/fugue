//! `fugue.pkg.json` manifest types.
//!
//! The manifest is the shared declarative format used by every (β) Fugue
//! extension kind. The struct is intentionally permissive on parse and
//! strict on [`crate::pkg::validate`] — round-tripping unknown reserved
//! fields (`signing`) is supported so Phase 2 trust work is non-breaking.

use serde::{Deserialize, Serialize};

/// A `fugue.pkg.json` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageManifest {
    /// Reverse-DNS identifier (e.g. `fugue.example.reverb`).
    pub id: String,

    /// Package semver.
    pub version: String,

    /// What kind of extension this manifest describes.
    pub kind: PackageKind,

    /// SPDX license identifier.
    pub license: String,

    /// Manifest authors. At least one required.
    #[serde(default)]
    pub authors: Vec<Author>,

    /// Short human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Project / docs URL.
    #[serde(default)]
    pub homepage: Option<String>,

    /// Surfaces this package supports running in. At least one required.
    #[serde(default)]
    pub targets: Vec<Target>,

    /// Declared runtime requirements.
    #[serde(default)]
    pub requires: Requires,

    /// Other Fugue packages this manifest depends on (`id@req` strings).
    #[serde(default)]
    pub deps: Vec<String>,

    /// Kind-specific entry point.
    pub entry: EntrySpec,

    /// Reserved signing metadata. Phase 1 accepts but does not enforce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing: Option<Signing>,
}

/// Extension kind discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    Module,
    Development,
    Invention,
    Skill,
    AgentDefinition,
    SamplePack,
}

impl PackageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PackageKind::Module => "module",
            PackageKind::Development => "development",
            PackageKind::Invention => "invention",
            PackageKind::Skill => "skill",
            PackageKind::AgentDefinition => "agent-definition",
            PackageKind::SamplePack => "sample-pack",
        }
    }
}

/// One manifest author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Author {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Surface the package can be loaded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    ClaudeCode,
    InGraphAgent,
}

/// Declared runtime requirements.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Requires {
    /// MCP tool ids this package expects to be available.
    #[serde(default, rename = "mcp-tools", skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<String>,

    /// Capability strings (see [`Capability`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Kind-specific entry point. The active variant must match
/// [`PackageManifest::kind`]; that invariant is enforced by validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum EntrySpec {
    Module { wasm: String },
    Invention { invention: String },
    Development { development: String },
    Skill { skill: String },
    AgentDefinition { definition: String },
    SamplePack { samples: String },
}

impl EntrySpec {
    /// The kind this entry variant corresponds to.
    pub fn kind(&self) -> PackageKind {
        match self {
            EntrySpec::Module { .. } => PackageKind::Module,
            EntrySpec::Invention { .. } => PackageKind::Invention,
            EntrySpec::Development { .. } => PackageKind::Development,
            EntrySpec::Skill { .. } => PackageKind::Skill,
            EntrySpec::AgentDefinition { .. } => PackageKind::AgentDefinition,
            EntrySpec::SamplePack { .. } => PackageKind::SamplePack,
        }
    }
}

/// Reserved signing block. Phase 1 accepts but does not verify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Signing {
    pub key: String,
    pub signature: String,
}

/// Parsed shape of a capability string.
///
/// Capabilities are declared as flat strings in the manifest so they remain
/// forward-compatible; this enum is the parsed form returned by
/// [`Capability::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Random,
    Time,
    FsRead(String),
    FsWrite(String),
    Net(String),
}

impl Capability {
    /// Parse a capability string. Returns `None` if the prefix is unknown
    /// or the body is empty for a scoped capability.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "random" => Some(Capability::Random),
            "time" => Some(Capability::Time),
            _ => {
                let (prefix, rest) = raw.split_once(':')?;
                match prefix {
                    "fs" => {
                        let (op, scope) = rest.split_once(':')?;
                        if scope.is_empty() {
                            return None;
                        }
                        match op {
                            "read" => Some(Capability::FsRead(scope.to_string())),
                            "write" => Some(Capability::FsWrite(scope.to_string())),
                            _ => None,
                        }
                    }
                    "net" => {
                        if rest.is_empty() {
                            None
                        } else {
                            Some(Capability::Net(rest.to_string()))
                        }
                    }
                    _ => None,
                }
            }
        }
    }
}

/// Parsed dependency reference (`id@requirement`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepRef {
    pub id: String,
    pub requirement: String,
}

impl DepRef {
    /// Parse a dep string of the form `id@requirement`. Both halves must be
    /// non-empty; the requirement is not parsed further here (the resolver
    /// ticket owns that).
    pub fn parse(raw: &str) -> Option<Self> {
        let (id, req) = raw.split_once('@')?;
        if id.is_empty() || req.is_empty() {
            return None;
        }
        Some(DepRef {
            id: id.to_string(),
            requirement: req.to_string(),
        })
    }
}
