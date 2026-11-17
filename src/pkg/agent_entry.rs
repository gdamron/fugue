//! File checks for an `agent` package: its definition file.
//!
//! An agent's manifest declares what it requires (`skills`, `policies`), so
//! the lockfile resolves them; the definition file holds only what the agent
//! is:
//!
//! ```json
//! { "system_prompt": "You are...", "default_tools": ["fugue.set_control"] }
//! ```
//!
//! - `system_prompt` (required): the persona, non-empty text.
//! - `default_tools` (optional, default none): a list of tool names.
//! - `default_model` (optional): a model id. When absent, the host's
//!   configured model applies, so a definition need not pin one that goes
//!   stale. Model ids are not checked here.
//! - `id` and `version` (optional): when present, must equal the manifest's.
//!
//! Any other field, `skills`, `policies` and `kind` included, is refused, so
//! a requirement written in the wrong file fails instead of being ignored.
//! Manifest validation cannot see the file, so installers and pack checkers
//! call [`check_agent_package`] once the package is on disk.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::manifest::{EntrySpec, PackageManifest};
use super::package_file::{self, PackageFileError};

/// The fields a definition file may hold.
const FIELDS: [&str; 5] = [
    "system_prompt",
    "default_tools",
    "default_model",
    "id",
    "version",
];

/// An agent package's definition: who the agent is and how it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    /// The persona prompt.
    pub system_prompt: String,
    /// Tool names the agent starts with, in order.
    pub default_tools: Vec<String>,
    /// The model the agent asks for; `None` leaves the host's choice.
    pub default_model: Option<String>,
}

/// Why an agent package was refused. Every variant names the path or field it
/// is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPackageError {
    /// The manifest is not an `agent` package.
    NotAgentPackage { id: String, kind: &'static str },
    /// The definition is outside the package, linked, or missing.
    Definition(PackageFileError),
    /// The definition exists but could not be read as text.
    Unreadable { path: PathBuf, reason: String },
    /// The definition is not valid JSON, or not a JSON object.
    InvalidJson { path: PathBuf, reason: String },
    /// A field the definition schema does not have.
    UnknownField { path: PathBuf, field: String },
    /// A required field is absent, or text that is empty.
    MissingField { path: PathBuf, field: &'static str },
    /// A field holds the wrong type of value.
    WrongType {
        path: PathBuf,
        field: &'static str,
        expected: &'static str,
    },
    /// `id` or `version` differs from the manifest's.
    Mismatch {
        path: PathBuf,
        field: &'static str,
        definition: String,
        manifest: String,
    },
}

impl fmt::Display for AgentPackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentPackageError::NotAgentPackage { id, kind } => {
                write!(f, "{id} is a `{kind}` package, not an `agent` package")
            }
            AgentPackageError::Definition(error) => write!(f, "agent definition {error}"),
            AgentPackageError::Unreadable { path, reason } => write!(
                f,
                "agent definition `{}` cannot be read: {reason}",
                path.display()
            ),
            AgentPackageError::InvalidJson { path, reason } => write!(
                f,
                "agent definition `{}` is not a valid JSON object: {reason}",
                path.display()
            ),
            AgentPackageError::UnknownField { path, field } => {
                let hint = match field.as_str() {
                    "skills" | "policies" => " (declare it on the package manifest)",
                    _ => "",
                };
                write!(
                    f,
                    "agent definition `{}` has unknown field `{field}`{hint}; it accepts \
                     `system_prompt`, `default_tools`, `default_model`, `id` and `version`",
                    path.display()
                )
            }
            AgentPackageError::MissingField { path, field } => write!(
                f,
                "agent definition `{}` field `{field}` is missing or empty",
                path.display()
            ),
            AgentPackageError::WrongType {
                path,
                field,
                expected,
            } => write!(
                f,
                "agent definition `{}` field `{field}` must be {expected}",
                path.display()
            ),
            AgentPackageError::Mismatch {
                path,
                field,
                definition,
                manifest,
            } => write!(
                f,
                "agent definition `{}` field `{field}` is `{definition}`, but the manifest's \
                 is `{manifest}`",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AgentPackageError {}

/// Check an `agent` package's definition file under `package_root`, returning
/// the parsed definition.
pub fn check_agent_package(
    package_root: &Path,
    manifest: &PackageManifest,
) -> Result<AgentDefinition, AgentPackageError> {
    let path = definition_path(package_root, manifest)?;
    let bytes =
        super::read_limit::read_document(&path).map_err(|err| AgentPackageError::Unreadable {
            path: path.clone(),
            reason: err.to_string(),
        })?;
    parse_definition(&path, &bytes, manifest)
}

/// The definition file `manifest` names under `package_root`: a regular file
/// inside the package, reached without links and outside `.git`.
pub(crate) fn definition_path(
    package_root: &Path,
    manifest: &PackageManifest,
) -> Result<PathBuf, AgentPackageError> {
    let EntrySpec::Agent { definition } = &manifest.entry else {
        return Err(AgentPackageError::NotAgentPackage {
            id: manifest.id.clone(),
            kind: manifest.kind.as_str(),
        });
    };
    package_file::locate(package_root, Path::new(definition)).map_err(AgentPackageError::Definition)
}

/// Parse the definition file's `bytes`, read from `path`, against
/// `manifest`.
pub(crate) fn parse_definition(
    path: &Path,
    bytes: &[u8],
    manifest: &PackageManifest,
) -> Result<AgentDefinition, AgentPackageError> {
    let invalid = |reason: String| AgentPackageError::InvalidJson {
        path: path.to_path_buf(),
        reason,
    };
    let value: Value = serde_json::from_slice(package_file::strip_bom(bytes))
        .map_err(|err| invalid(err.to_string()))?;
    let Value::Object(fields) = value else {
        return Err(invalid("the top level must be an object".to_string()));
    };
    if let Some(field) = fields.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(AgentPackageError::UnknownField {
            path: path.to_path_buf(),
            field: field.clone(),
        });
    }
    let fields = Fields { path, fields };
    fields.must_match("id", &manifest.id)?;
    fields.must_match("version", &manifest.version)?;
    let system_prompt = fields
        .text("system_prompt")?
        .ok_or_else(|| fields.missing("system_prompt"))?;
    let default_model = fields.text("default_model")?;
    if fields.fields.contains_key("default_model") && default_model.is_none() {
        return Err(fields.missing("default_model"));
    }
    Ok(AgentDefinition {
        system_prompt,
        default_tools: fields.tools()?,
        default_model,
    })
}

/// A definition's fields, read with errors naming the file.
struct Fields<'a> {
    path: &'a Path,
    fields: Map<String, Value>,
}

impl Fields<'_> {
    fn missing(&self, field: &'static str) -> AgentPackageError {
        AgentPackageError::MissingField {
            path: self.path.to_path_buf(),
            field,
        }
    }

    fn wrong_type(&self, field: &'static str, expected: &'static str) -> AgentPackageError {
        AgentPackageError::WrongType {
            path: self.path.to_path_buf(),
            field,
            expected,
        }
    }

    /// A text field: `None` when absent or only whitespace.
    fn text(&self, field: &'static str) -> Result<Option<String>, AgentPackageError> {
        match self.fields.get(field) {
            None => Ok(None),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(self.wrong_type(field, "text")),
        }
    }

    /// `default_tools`: a list of non-empty tool names, empty when absent.
    fn tools(&self) -> Result<Vec<String>, AgentPackageError> {
        const FIELD: &str = "default_tools";
        let expected = "a list of tool names";
        let Some(value) = self.fields.get(FIELD) else {
            return Ok(Vec::new());
        };
        let Value::Array(items) = value else {
            return Err(self.wrong_type(FIELD, expected));
        };
        items
            .iter()
            .map(|item| match item {
                Value::String(name) if !name.trim().is_empty() => Ok(name.clone()),
                _ => Err(self.wrong_type(FIELD, expected)),
            })
            .collect()
    }

    /// When `field` is present it must be the text `manifest`.
    fn must_match(&self, field: &'static str, manifest: &str) -> Result<(), AgentPackageError> {
        match self.fields.get(field) {
            None => Ok(()),
            Some(Value::String(definition)) if definition == manifest => Ok(()),
            Some(Value::String(definition)) => Err(AgentPackageError::Mismatch {
                path: self.path.to_path_buf(),
                field,
                definition: definition.clone(),
                manifest: manifest.to_string(),
            }),
            Some(_) => Err(self.wrong_type(field, "text")),
        }
    }
}

#[cfg(test)]
#[path = "agent_entry_tests.rs"]
mod tests;
