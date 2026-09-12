//! Validate transitive developments and assets and collect closure fingerprints.

use super::*;
use std::collections::BTreeSet;

impl ContentRoots {
    pub(super) fn walk_document(
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
        let key = format!("package:{}@{}", manifest.id, manifest.version);
        // One hash per package per closure: a multi-zone sample instrument names
        // the same sample library once per zone.
        if closure.fingerprints.contains_key(&key) {
            return Ok(());
        }
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
        closure.fingerprints.insert(key, integrity);
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Closure {
    pub(super) visits: std::rc::Rc<std::cell::Cell<usize>>,
    pub(super) active: BTreeSet<PathBuf>,
    pub(super) fingerprints: BTreeMap<String, String>,
    pub(super) dependencies: Vec<ContentRef>,
    pub(super) assets: Vec<String>,
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
