//! Resolve daemon-local content references and prepare validated import snapshots.

use super::*;
use crate::{
    pkg::{self, Lockfile, PackageAudioRef, PackageManifest},
    Invention,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ContentError>;
const LIST_BYTES: usize = 65_536;
const DETAIL_BYTES: usize = 262_144;
const MAX_FILES: usize = 10_000;

/// The existing daemon-owned roots. Clients never supply alternate search paths.
#[derive(Debug, Clone)]
pub struct ContentRoots {
    pub packages: PathBuf,
    pub workspace: PathBuf,
}
impl ContentRoots {
    /// Resolve the same package cache and inventions directory used by the host.
    pub fn from_environment() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or_else(|| {
                err(
                    "catalog_unavailable",
                    "Could not determine the daemon workspace",
                )
            })?;
        Ok(Self {
            packages: pkg::default_packages_dir().map_err(|e| err("catalog_unavailable", e))?,
            workspace: PathBuf::from(home).join(".fugue/inventions"),
        })
    }

    /// Revalidate an exact development reference and preserve its declaring path.
    pub fn load_development(&self, reference: &ContentRef) -> Result<Invention> {
        let _guard = package_read_guard(&self.packages)?;
        validate_reference(reference)?;
        let resolved = self.inspect(reference)?;
        if resolved.entry.kind != ContentKind::Development {
            return Err(err("kind_mismatch", "Select a development reference"));
        }
        let mut document = resolved.document;
        self.prepare_definition(&mut document, 0)?;
        if self.inspect(reference)?.fingerprints != resolved.fingerprints {
            return Err(err(
                "stale_reference",
                "Content changed while importing; inspect and retry",
            ));
        }
        Ok(document)
    }

    fn prepare_definition(&self, document: &mut Invention, depth: usize) -> Result<()> {
        if depth > 64 {
            return Err(err(
                "invalid_content",
                "Development nesting exceeds 64 levels",
            ));
        }
        let parent = document
            .source_path
            .as_deref()
            .and_then(Path::parent)
            .ok_or_else(|| err("invalid_content", "Missing catalog source path"))?
            .to_path_buf();
        for development in &mut document.developments {
            let mut child = if let Some(reference) = &development.reference {
                self.inspect(reference)?.document
            } else if let Some(path) = &development.path {
                Invention::from_file(
                    parent
                        .join(path)
                        .to_str()
                        .ok_or_else(|| err("invalid_content", "Non-UTF-8 development path"))?,
                )
                .map_err(|e| err("invalid_content", e))?
            } else {
                let mut child = development
                    .definition
                    .as_deref()
                    .cloned()
                    .ok_or_else(|| err("invalid_content", "Missing development definition"))?;
                child.source_path = document.source_path.clone();
                child
            };
            self.prepare_definition(&mut child, depth + 1)?;
            development.path = None;
            development.reference = None;
            development.definition = Some(Box::new(child));
        }
        crate::invention::builder::resolve_json_assets(document)
            .map_err(|e| err("invalid_content", e))?;
        // Resolve audio refs in the snapshot, so the legacy loader cannot reselect
        // a different version or write a lockfile into an immutable package.
        for module in &mut document.modules {
            for value in crate::invention::audio_assets::module_asset_values(&mut module.config) {
                if let Some(reference) = value.as_str().and_then(PackageAudioRef::parse) {
                    let pin = self.dependency_pin(&parent, &reference.id)?;
                    let resolved =
                        pkg::resolve_package_asset(&reference, &self.packages, pin.as_deref())
                            .map_err(|e| err("dependency_not_found", e))?;
                    *value = serde_json::Value::String(
                        contained(&resolved.install_dir, &resolved.file)?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        Ok(())
    }

    fn dependency_pin(&self, parent: &Path, id: &str) -> Result<Option<String>> {
        if let Some(pin) = read_lock(&parent.join(pkg::LOCKFILE_NAME))?
            .and_then(|lock| lock.packages.get(id).map(|p| p.version.clone()))
        {
            return Ok(Some(pin));
        }
        let packages = canonical(&self.packages, "content_not_found")?;
        if let Ok(relative) = parent.strip_prefix(packages) {
            let mut parts = relative.iter();
            if let (Some(owner), Some(version)) = (
                parts.next().and_then(|s| s.to_str()),
                parts.next().and_then(|s| s.to_str()),
            ) {
                if let Some(receipt) = read_receipt(&self.packages, owner, version)? {
                    return Ok(receipt
                        .package
                        .dependencies
                        .iter()
                        .filter_map(|d| d.rsplit_once('@'))
                        .find(|(dep, _)| *dep == id)
                        .map(|(_, version)| version.into()));
                }
            }
        }
        Ok(None)
    }

    fn inspect(&self, reference: &ContentRef) -> Result<Resolved> {
        let mut closure = Closure::default();
        let (path, root, manifest) = self.reference_path(reference)?;
        let document = self.walk_document(&path, &root, manifest.as_ref(), &mut closure, 0)?;
        let (id, version, source, summary) = match reference {
            ContentRef::Package { package, version } => (
                package.clone(),
                version.clone(),
                ContentSource::Installed,
                manifest
                    .as_ref()
                    .and_then(|m| m.description.clone())
                    .unwrap_or_default(),
            ),
            ContentRef::Workspace {
                workspace_path,
                revision,
            } => {
                let computed = revision_of(&closure.fingerprints)?;
                if !revision.is_empty() && *revision != computed {
                    return Err(err(
                        "stale_reference",
                        "Workspace content changed; list and inspect its new revision",
                    ));
                }
                (
                    format!("workspace:{workspace_path}"),
                    computed,
                    ContentSource::Workspace,
                    document.description.clone().unwrap_or_default(),
                )
            }
        };
        let reference = match reference {
            ContentRef::Workspace { workspace_path, .. } => ContentRef::Workspace {
                workspace_path: workspace_path.clone(),
                revision: version.clone(),
            },
            _ => reference.clone(),
        };
        let kind = if document.is_development() {
            ContentKind::Development
        } else {
            ContentKind::Invention
        };
        if let Some(m) = &manifest {
            let expected = if kind == ContentKind::Development {
                pkg::PackageKind::Development
            } else {
                pkg::PackageKind::Invention
            };
            if m.kind != expected {
                return Err(err(
                    "kind_mismatch",
                    "Manifest kind disagrees with the entry document",
                ));
            }
        }
        let source = if let ContentRef::Package { package, version } = &reference {
            read_receipt(&self.packages, package, version)?
                .filter(|r| r.bundled)
                .map_or(source, |_| ContentSource::Bundled)
        } else {
            source
        };
        let entry = ContentEntry {
            name: document.title.clone().unwrap_or_else(|| id.clone()),
            id,
            version,
            kind,
            source,
            summary: truncate(&summary, 512),
            reference,
        };
        Ok(Resolved {
            entry,
            document,
            dependencies: closure.dependencies,
            assets: closure.assets,
            fingerprints: closure.fingerprints,
        })
    }

    fn reference_path(
        &self,
        reference: &ContentRef,
    ) -> Result<(PathBuf, PathBuf, Option<PackageManifest>)> {
        match reference {
            ContentRef::Package { package, version } => {
                validate_package(package, version)?;
                // Resolve the manifest through the existing installed-version resolver.
                let resolved = pkg::resolve_package_asset(
                    &PackageAudioRef {
                        id: package.clone(),
                        requirement: format!("={version}"),
                        path: "fugue.pkg.json".into(),
                    },
                    &self.packages,
                    Some(version),
                )
                .map_err(|e| err("content_not_found", e))?;
                if resolved.version.to_string() != *version {
                    return Err(err(
                        "content_not_found",
                        "The exact package version is not installed",
                    ));
                }
                let root = canonical(&resolved.install_dir, "content_not_found")?;
                contained(&self.packages, &root)?;
                let manifest_path = contained(&root, &resolved.file)?;
                let manifest =
                    pkg::parse_path(&manifest_path).map_err(|e| err("invalid_content", e))?;
                if manifest.id != *package || manifest.version != *version {
                    return Err(err(
                        "integrity_mismatch",
                        "Manifest identity differs from its installed coordinate",
                    ));
                }
                let entry = match &manifest.entry {
                    pkg::EntrySpec::Development { development } => development,
                    pkg::EntrySpec::Invention { invention } => invention,
                    _ => return Err(err("kind_mismatch", "Package is not musical content")),
                };
                let path = contained(&root, &root.join(entry)).map_err(missing_entry)?;
                Ok((path, root, Some(manifest)))
            }
            ContentRef::Workspace {
                workspace_path,
                revision,
            } => {
                validate_workspace_path(workspace_path)?;
                if !revision.is_empty() {
                    validate_revision(revision)?;
                }
                let path = contained(&self.workspace, &self.workspace.join(workspace_path))
                    .map_err(missing_entry)?;
                Ok((path, canonical(&self.workspace, "content_not_found")?, None))
            }
        }
    }
}

struct Resolved {
    entry: ContentEntry,
    document: Invention,
    dependencies: Vec<ContentRef>,
    assets: Vec<String>,
    fingerprints: BTreeMap<String, String>,
}

mod catalog;
mod closure;
mod storage;

pub use catalog::ContentCatalog;
use closure::Closure;
use storage::{
    canonical, contained, directory, hash, package_read_guard, read_file, read_lock, read_receipt,
    revision_of, workspace_refs,
};
pub use storage::{receipt_path, ContentReceipt};

fn err(code: &str, message: impl ToString) -> ContentError {
    ContentError::new(code, message)
}
fn io_error(e: std::io::Error) -> ContentError {
    err("catalog_unavailable", e)
}
fn validate_package(package: &str, version: &str) -> Result<()> {
    if package.is_empty()
        || package == "."
        || package == ".."
        || !package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || semver::Version::parse(version).is_err()
    {
        return Err(err(
            "invalid_request",
            "Use a valid package ID and exact SemVer",
        ));
    }
    Ok(())
}
fn validate_workspace_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\\')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || Path::new(path).is_absolute()
    {
        return Err(err(
            "invalid_request",
            "Use a normalized workspace-relative path",
        ));
    }
    Ok(())
}
fn validate_revision(revision: &str) -> Result<()> {
    if !revision.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }) {
        return Err(err(
            "invalid_request",
            "Use the sha256 revision returned by list",
        ));
    }
    Ok(())
}
fn validate_reference(reference: &ContentRef) -> Result<()> {
    match reference {
        ContentRef::Package { package, version } => validate_package(package, version),
        ContentRef::Workspace {
            workspace_path,
            revision,
        } => {
            validate_workspace_path(workspace_path)?;
            validate_revision(revision)
        }
    }
}
fn truncate(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}
fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
use std::fs;
#[cfg(test)]
mod tests;

fn missing_entry(mut error: ContentError) -> ContentError {
    if error.code == "dependency_not_found" {
        error.code = "content_not_found".into();
    }
    error
}
