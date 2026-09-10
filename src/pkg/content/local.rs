use super::*;
use crate::{
    pkg::{self, Lockfile, PackageAudioRef, PackageManifest},
    Invention,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ContentError>;
const LIST_BYTES: usize = 65_536;
const DETAIL_BYTES: usize = 262_144;
const FILE_BYTES: u64 = 16 * 1024 * 1024;
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

    fn walk_document(
        &self,
        path: &Path,
        root: &Path,
        manifest: Option<&PackageManifest>,
        closure: &mut Closure,
        depth: usize,
    ) -> Result<Invention> {
        closure.visits.set(closure.visits.get() + 1);
        if depth > 64 || closure.visits.get() > MAX_FILES || closure.fingerprints.len() >= MAX_FILES
        {
            return Err(err(
                "invalid_content",
                "Content dependency closure exceeds 64 levels or 10000 files",
            ));
        }
        let path = contained(root, path)?;
        if !closure.active.insert(path.clone()) {
            return Err(err("invalid_content", "Development dependency cycle"));
        }
        let bytes = read_file(&path)?;
        let mut document: Invention =
            serde_json::from_slice(&bytes).map_err(|e| err("invalid_content", e))?;
        document.source_path = Some(path.clone());
        self.fingerprint_file(&path, root, &bytes, manifest, closure)?;
        if let Some(m) = manifest.filter(|m| match &m.entry {
            pkg::EntrySpec::Development { development } => root.join(development) == path,
            pkg::EntrySpec::Invention { invention } => root.join(invention) == path,
            _ => false,
        }) {
            self.package_integrity(root, m, closure)?;
            for dep in &m.deps {
                let dep = pkg::DepRef::parse(dep)
                    .ok_or_else(|| err("invalid_content", "Invalid package dependency"))?;
                let pin = self.dependency_pin(root, &dep.id)?;
                let resolved = pkg::resolve_package_asset(
                    &PackageAudioRef {
                        id: dep.id.clone(),
                        requirement: dep.requirement,
                        path: "fugue.pkg.json".into(),
                    },
                    &self.packages,
                    pin.as_deref(),
                )
                .map_err(|e| err("dependency_not_found", e))?;
                let dep_manifest =
                    pkg::parse_path(&resolved.file).map_err(|e| err("invalid_content", e))?;
                self.package_integrity(&resolved.install_dir, &dep_manifest, closure)?;
                if matches!(
                    dep_manifest.kind,
                    pkg::PackageKind::Development | pkg::PackageKind::Invention
                ) {
                    let reference = ContentRef::Package {
                        package: dep.id,
                        version: resolved.version.to_string(),
                    };
                    let (child, child_root, child_manifest) = self.reference_path(&reference)?;
                    self.walk_document(
                        &child,
                        &child_root,
                        child_manifest.as_ref(),
                        closure,
                        depth + 1,
                    )?;
                    push_unique(&mut closure.dependencies, reference);
                }
            }
        }
        self.walk_inline(&document, &path, root, manifest, closure, depth)?;
        closure.active.remove(&path);
        Ok(document)
    }

    fn walk_inline(
        &self,
        document: &Invention,
        path: &Path,
        root: &Path,
        manifest: Option<&PackageManifest>,
        closure: &mut Closure,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err(err(
                "invalid_content",
                "Development nesting exceeds 64 levels",
            ));
        }
        let parent = path.parent().unwrap_or(root);
        let mut aliases = BTreeSet::new();
        let primitives = crate::ModuleRegistry::default();
        for dev in &document.developments {
            if !aliases.insert(&dev.name) || primitives.has_type(&dev.name) {
                return Err(err(
                    "duplicate_type_name",
                    format!("Choose another development alias: {}", dev.name),
                ));
            }
            if usize::from(dev.path.is_some())
                + usize::from(dev.definition.is_some())
                + usize::from(dev.reference.is_some())
                != 1
            {
                return Err(err(
                    "invalid_content",
                    "Development requires exactly one of path, definition or ref",
                ));
            }
            if let Some(reference) = &dev.reference {
                validate_reference(reference)?;
                if let Some(m) = manifest {
                    let ContentRef::Package { package, version } = reference else {
                        return Err(err(
                            "outside_root",
                            "Package dependencies cannot reference workspace files",
                        ));
                    };
                    let declared = m
                        .deps
                        .iter()
                        .filter_map(|s| pkg::DepRef::parse(s))
                        .any(|d| {
                            d.id == *package
                                && semver::VersionReq::parse(&d.requirement).is_ok_and(|r| {
                                    r.matches(&semver::Version::parse(version).unwrap())
                                })
                        });
                    if !declared {
                        return Err(err(
                            "dependency_not_found",
                            "Cross-package imports must be declared in manifest deps",
                        ));
                    }
                }
                let (child, child_root, child_manifest) = self.reference_path(reference)?;
                let mut child_closure = Closure {
                    active: closure.active.clone(),
                    visits: closure.visits.clone(),
                    ..Default::default()
                };
                let child_doc = self.walk_document(
                    &child,
                    &child_root,
                    child_manifest.as_ref(),
                    &mut child_closure,
                    depth + 1,
                )?;
                if !child_doc.is_development() {
                    return Err(err(
                        "kind_mismatch",
                        "Imported content is not a development",
                    ));
                }
                if let ContentRef::Workspace { revision, .. } = reference {
                    if revision_of(&child_closure.fingerprints)? != *revision {
                        return Err(err(
                            "stale_reference",
                            "A workspace dependency changed; select its new reference",
                        ));
                    }
                }
                merge_closure(closure, child_closure);
                push_unique(&mut closure.dependencies, reference.clone());
            } else if let Some(relative) = &dev.path {
                let child = contained(root, &parent.join(relative))?;
                let mut child_closure = Closure {
                    active: closure.active.clone(),
                    visits: closure.visits.clone(),
                    ..Default::default()
                };
                let child_doc =
                    self.walk_document(&child, root, manifest, &mut child_closure, depth + 1)?;
                if !child_doc.is_development() {
                    return Err(err("kind_mismatch", "Imported file is not a development"));
                }
                if manifest.is_none() {
                    let workspace = canonical(&self.workspace, "content_not_found")?;
                    let reference = ContentRef::Workspace {
                        workspace_path: child
                            .strip_prefix(workspace)
                            .map_err(|e| err("outside_root", e))?
                            .to_string_lossy()
                            .into_owned(),
                        revision: revision_of(&child_closure.fingerprints)?,
                    };
                    push_unique(&mut closure.dependencies, reference);
                }
                merge_closure(closure, child_closure);
            } else if let Some(inline) = &dev.definition {
                self.walk_inline(inline, path, root, manifest, closure, depth + 1)?;
            }
        }
        for asset in document.assets.values() {
            self.local_asset(&asset.path, parent, root, manifest, closure)?;
        }
        let mut expanded = document.clone();
        expanded.source_path = Some(path.to_path_buf());
        crate::invention::builder::resolve_json_assets(&mut expanded)
            .map_err(|e| err("invalid_content", e))?;
        for module in &expanded.modules {
            if !primitives.has_type(&module.module_type) && !aliases.contains(&module.module_type) {
                return Err(err(
                    "invalid_content",
                    format!("Unknown module type: {}", module.module_type),
                ));
            }
            let mut slots = Vec::new();
            if let Some(asset) = module.config.get("asset") {
                slots.push(asset);
            }
            for key in ["samples", "zones"] {
                if let Some(values) = module.config.get(key).and_then(|v| v.as_array()) {
                    slots.extend(values.iter().filter_map(|v| v.get("asset")));
                }
            }
            for slot in slots {
                let asset: pkg::AudioAssetRef =
                    serde_json::from_value(slot.clone()).map_err(|e| err("invalid_content", e))?;
                match asset {
                    pkg::AudioAssetRef::Text(text) => {
                        if let Some(reference) = PackageAudioRef::parse(&text) {
                            if let Some(m) = manifest {
                                if !m
                                    .deps
                                    .iter()
                                    .filter_map(|d| pkg::DepRef::parse(d))
                                    .any(|d| d.id == reference.id)
                                {
                                    return Err(err("dependency_not_found", "Package audio dependencies must be declared in manifest deps"));
                                }
                            }
                            let pin = self.dependency_pin(parent, &reference.id)?;
                            let resolved = pkg::resolve_package_asset(
                                &reference,
                                &self.packages,
                                pin.as_deref(),
                            )
                            .map_err(|e| err("dependency_not_found", e))?;
                            contained(&resolved.install_dir, &resolved.file)?;
                            let m = pkg::parse_path(&resolved.install_dir.join("fugue.pkg.json"))
                                .map_err(|e| err("invalid_content", e))?;
                            self.package_integrity(&resolved.install_dir, &m, closure)?;
                            push_unique(&mut closure.assets, text);
                        } else {
                            self.local_asset(&text, parent, root, manifest, closure)?;
                        }
                    }
                    pkg::AudioAssetRef::Local { path } => {
                        self.local_asset(&path, parent, root, manifest, closure)?
                    }
                }
            }
        }
        Ok(())
    }

    fn local_asset(
        &self,
        relative: &str,
        parent: &Path,
        root: &Path,
        manifest: Option<&PackageManifest>,
        closure: &mut Closure,
    ) -> Result<()> {
        let path = contained(root, &parent.join(relative))?;
        let bytes = read_file(&path)?;
        self.fingerprint_file(&path, root, &bytes, manifest, closure)?;
        push_unique(&mut closure.assets, relative.into());
        Ok(())
    }

    fn fingerprint_file(
        &self,
        path: &Path,
        root: &Path,
        bytes: &[u8],
        manifest: Option<&PackageManifest>,
        closure: &mut Closure,
    ) -> Result<()> {
        if manifest.is_none()
            && path.starts_with(
                canonical(&self.workspace, "content_not_found")
                    .unwrap_or_else(|_| self.workspace.clone()),
            )
        {
            let workspace = canonical(&self.workspace, "content_not_found")?;
            let relative = path
                .strip_prefix(workspace)
                .map_err(|e| err("outside_root", e))?;
            closure
                .fingerprints
                .insert(format!("file:{}", relative.to_string_lossy()), hash(bytes));
        } else if manifest.is_none() {
            // Local files in a package are covered by its complete integrity hash.
            contained(root, path)?;
        }
        Ok(())
    }

    fn package_integrity(
        &self,
        root: &Path,
        manifest: &PackageManifest,
        closure: &mut Closure,
    ) -> Result<()> {
        contained(&self.packages, root)?;
        let integrity = pkg::compute_integrity(root).map_err(|e| err("catalog_unavailable", e))?;
        if let Some(receipt) = read_receipt(&self.packages, &manifest.id, &manifest.version)? {
            if receipt.package.integrity != integrity {
                return Err(err("integrity_mismatch", "Installed package differs from its install receipt; reinstall the original version"));
            }
        }
        for location in [
            root.join(pkg::LOCKFILE_NAME),
            self.packages
                .parent()
                .unwrap_or(&self.packages)
                .join(pkg::LOCKFILE_NAME),
        ] {
            if let Some(lock) = read_lock(&location)? {
                if let Some(pin) = lock
                    .packages
                    .get(&manifest.id)
                    .filter(|p| p.version == manifest.version && !p.integrity.is_empty())
                {
                    if pin.integrity != integrity {
                        return Err(err(
                            "integrity_mismatch",
                            "Installed package differs from its lock integrity",
                        ));
                    }
                }
            }
        }
        closure.fingerprints.insert(
            format!("package:{}@{}", manifest.id, manifest.version),
            integrity,
        );
        Ok(())
    }
}

#[derive(Default)]
struct Closure {
    visits: std::rc::Rc<std::cell::Cell<usize>>,
    active: BTreeSet<PathBuf>,
    fingerprints: BTreeMap<String, String>,
    dependencies: Vec<ContentRef>,
    assets: Vec<String>,
}
struct Resolved {
    entry: ContentEntry,
    document: Invention,
    dependencies: Vec<ContentRef>,
    assets: Vec<String>,
    fingerprints: BTreeMap<String, String>,
}

mod catalog;
pub use catalog::ContentCatalog;

fn err(code: &str, message: impl ToString) -> ContentError {
    ContentError::new(code, message)
}
fn io_error(e: std::io::Error) -> ContentError {
    err("catalog_unavailable", e)
}
fn canonical(path: &Path, code: &str) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|e| {
        err(
            if e.kind() == std::io::ErrorKind::NotFound {
                code
            } else {
                "catalog_unavailable"
            },
            e,
        )
    })
}
fn contained(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = canonical(root, "content_not_found")?;
    let path = canonical(path, "dependency_not_found")?;
    if !path.starts_with(root) {
        return Err(err("outside_root", "Content escapes its declaring root"));
    }
    Ok(path)
}
fn read_file(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path).map_err(io_error)?.len() > FILE_BYTES {
        return Err(err("invalid_content", "Content file exceeds 16 MiB"));
    }
    fs::read(path).map_err(io_error)
}
fn read_lock(path: &Path) -> Result<Option<Lockfile>> {
    match fs::read(path) {
        Ok(bytes) => Lockfile::from_slice(&bytes)
            .map(Some)
            .map_err(|e| err("invalid_content", e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}
fn directory(path: &Path) -> Result<Vec<fs::DirEntry>> {
    match fs::read_dir(path) {
        Ok(entries) => {
            let mut entries = entries
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(io_error)?;
            entries.sort_by_key(|e| e.file_name());
            Ok(entries)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(io_error(e)),
    }
}
fn workspace_refs(root: &Path, path: &Path, out: &mut Vec<ContentRef>, depth: usize) -> Result<()> {
    if depth > 64 || out.len() > MAX_FILES {
        return Err(err(
            "catalog_unavailable",
            "Workspace enumeration limit exceeded",
        ));
    }
    for entry in directory(path)? {
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_dir() {
            workspace_refs(root, &entry.path(), out, depth + 1)?;
        } else if entry.path().extension().is_some_and(|e| e == "json") {
            let path = entry.path();
            // JSON assets are not inventions. Malformed invention candidates remain diagnostics.
            if let Ok(bytes) = contained(root, &path).and_then(|path| read_file(&path)) {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if value.get("modules").is_none() || value.get("connections").is_none() {
                        continue;
                    }
                }
            }
            out.push(ContentRef::Workspace {
                workspace_path: path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .ok_or_else(|| err("invalid_content", "Workspace paths must be UTF-8"))?
                    .into(),
                revision: String::new(),
            });
        }
    }
    Ok(())
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
fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn revision_of(pairs: &BTreeMap<String, String>) -> Result<String> {
    Ok(hash(
        &serde_json::to_vec(&pairs.iter().collect::<Vec<_>>())
            .map_err(|e| err("invalid_content", e))?,
    ))
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
mod tests;

fn missing_entry(mut error: ContentError) -> ContentError {
    if error.code == "dependency_not_found" {
        error.code = "content_not_found".into();
    }
    error
}

/// Existing lock metadata retained for every installed version, with release provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentReceipt {
    pub package: pkg::LockedPackage,
    pub bundled: bool,
}

/// Metadata lives outside immutable package bytes and does not participate in lookup.
pub fn receipt_path(packages: &Path, id: &str, version: &str) -> PathBuf {
    packages
        .join(".receipts")
        .join(id)
        .join(format!("{version}.json"))
}
fn read_receipt(packages: &Path, id: &str, version: &str) -> Result<Option<ContentReceipt>> {
    match fs::read(receipt_path(packages, id, version)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| err("invalid_content", e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}
fn package_read_guard(packages: &Path) -> Result<Option<fs::File>> {
    if !packages.try_exists().map_err(io_error)? {
        return Ok(None);
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(packages.join(".catalog.lock"))
        .map_err(io_error)?;
    fs2::FileExt::lock_shared(&lock).map_err(io_error)?;
    Ok(Some(lock))
}

fn merge_closure(parent: &mut Closure, child: Closure) {
    parent.fingerprints.extend(child.fingerprints);
    for reference in child.dependencies {
        push_unique(&mut parent.dependencies, reference);
    }
    for asset in child.assets {
        push_unique(&mut parent.assets, asset);
    }
}
