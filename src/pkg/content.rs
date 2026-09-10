//! Stable, bounded discovery and references for daemon-local musical content.

use serde::{Deserialize, Serialize};

/// A portable exact package coordinate or a revision of daemon workspace content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(untagged, deny_unknown_fields)]
pub enum ContentRef {
    /// An exact installed package version; never a SemVer requirement.
    Package { package: String, version: String },
    /// A file and dependency-closure revision relative to the daemon workspace.
    Workspace {
        workspace_path: String,
        revision: String,
    },
}

/// Available musical content classification, independent of registered types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Development,
    Invention,
}

/// Delivery provenance; it never changes package identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ContentSource {
    Bundled,
    Installed,
    Workspace,
}

/// Compact catalog entry whose reference can be copied into an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ContentEntry {
    pub id: String,
    pub version: String,
    pub kind: ContentKind,
    pub name: String,
    pub summary: String,
    pub source: ContentSource,
    #[serde(rename = "ref")]
    pub reference: ContentRef,
}

/// A bounded request against a single catalog snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContentListQuery {
    pub schema_version: u32,
    pub kind: Option<ContentKind>,
    pub source: Option<ContentSource>,
    pub id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub cursor: Option<String>,
}
fn default_limit() -> usize {
    20
}
impl Default for ContentListQuery {
    fn default() -> Self {
        Self {
            schema_version: 1,
            kind: None,
            source: None,
            id: None,
            limit: 20,
            cursor: None,
        }
    }
}

/// Inspect an exact reference without loading it into the running graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContentDetailQuery {
    pub schema_version: u32,
    #[serde(rename = "ref")]
    pub reference: ContentRef,
}

/// Recoverable content error, shared by tools, imports and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ContentError {
    pub code: String,
    pub message: String,
}
impl ContentError {
    pub(crate) fn new(code: &str, message: impl ToString) -> Self {
        Self {
            code: code.into(),
            message: message.to_string(),
        }
    }
}
impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ContentError {}

/// A page of entries and bounded diagnostics from the same generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ContentPage {
    pub schema_version: u32,
    pub generation: String,
    pub items: Vec<ContentEntry>,
    pub next_cursor: Option<String>,
    pub diagnostics: Vec<ContentError>,
    pub diagnostics_truncated: bool,
}

/// Authored interface, preserving alias targets and fan-out declarations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ContentInterface {
    pub inputs: Vec<crate::invention::format::DevelopmentInput>,
    pub outputs: Vec<crate::invention::format::DevelopmentOutput>,
    pub controls: Vec<crate::invention::format::DevelopmentControl>,
}

/// Bounded details; the internal module graph is deliberately omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ContentDetail {
    pub schema_version: u32,
    pub generation: String,
    pub entry: ContentEntry,
    pub interface: Option<ContentInterface>,
    pub dependencies: Vec<ContentRef>,
    pub assets: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
mod local;
#[cfg(not(target_arch = "wasm32"))]
pub use local::{receipt_path, ContentCatalog, ContentReceipt, ContentRoots};
