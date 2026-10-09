//! Lists the local package cache. Reads disk on every call so an install by
//! any process is visible to the next list; never contacts a registry.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::{
    PackageInfo, PackageList, PackageListQuery, PackageSource, MAX_PACKAGE_DIAGNOSTICS,
    MAX_PACKAGE_LIST_BYTES, MAX_PACKAGE_SUMMARY_BYTES, PACKAGE_SCHEMA_VERSION,
};
use crate::pkg::content::{receipt_path, ContentError, ContentReceipt, ContentRef};
use crate::pkg::PackageKind;
use crate::ModuleRegistry;

/// Most installed package versions a listing scans before refusing.
const MAX_PACKAGE_CANDIDATES: usize = 10_000;
const MANIFEST_NAME: &str = "fugue.pkg.json";

type Result<T> = std::result::Result<T, ContentError>;

fn error(code: &str, message: impl Into<String>) -> ContentError {
    ContentError {
        code: code.to_string(),
        message: truncate(&message.into()),
    }
}

/// Cut `text` to [`MAX_PACKAGE_SUMMARY_BYTES`] on a character boundary.
fn truncate(text: &str) -> String {
    let mut end = text.len().min(MAX_PACKAGE_SUMMARY_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl PackageList {
    /// One page of built-in modules plus every package installed under
    /// `packages_dir`, sorted by ID then SemVer.
    ///
    /// A missing cache is empty; an unreadable one is `catalog_unavailable`.
    /// A version directory whose manifest is invalid becomes a diagnostic,
    /// never a usable entry. Pages stop before [`MAX_PACKAGE_LIST_BYTES`];
    /// a cursor expires (`stale_cursor`) once the inventory changes.
    pub fn local(
        registry: &ModuleRegistry,
        packages_dir: &Path,
        query: &PackageListQuery,
    ) -> Result<Self> {
        if query.schema_version != PACKAGE_SCHEMA_VERSION || !(1..=100).contains(&query.limit) {
            return Err(error(
                "invalid_request",
                "Use schema_version 1 and limit 1–100",
            ));
        }
        let (mut entries, diagnostics) = scan(packages_dir)?;
        entries.push(PackageInfo::built_in(registry));
        entries.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| compare_versions(a, b)));
        let matching: Vec<PackageInfo> = entries
            .into_iter()
            .filter(|entry| {
                query.kind.is_none_or(|kind| entry.kind == kind)
                    && query.source.is_none_or(|source| entry.source == source)
                    && query.id.as_ref().is_none_or(|id| entry.id == *id)
            })
            .map(|entry| if query.detail { entry } else { entry.compact() })
            .collect();
        let prefix = cursor_prefix(query, &matching);
        let offset = match &query.cursor {
            None => 0,
            Some(cursor) => cursor
                .strip_prefix(&prefix)
                .and_then(|offset| offset.parse::<usize>().ok())
                .filter(|offset| *offset <= matching.len())
                .ok_or_else(|| error("stale_cursor", "Restart list without a cursor"))?,
        };
        let diagnostics_truncated = diagnostics.len() > MAX_PACKAGE_DIAGNOSTICS;
        let mut page = PackageList {
            schema_version: PACKAGE_SCHEMA_VERSION,
            packages: Vec::new(),
            next_cursor: None,
            diagnostics: diagnostics
                .into_iter()
                .take(MAX_PACKAGE_DIAGNOSTICS)
                .collect(),
            diagnostics_truncated,
        };
        let mut bytes = encoded_len(&page)?;
        for (index, entry) in matching.iter().enumerate().skip(offset) {
            let entry_bytes = encoded_len(entry)? + 1;
            if page.packages.len() == query.limit || bytes + entry_bytes > MAX_PACKAGE_LIST_BYTES {
                if page.packages.is_empty() {
                    return Err(error(
                        "response_too_large",
                        format!(
                            "{}@{} exceeds the package list cap",
                            entry.id, entry.version
                        ),
                    ));
                }
                page.next_cursor = Some(format!("{prefix}{index}"));
                break;
            }
            bytes += entry_bytes;
            page.packages.push(entry.clone());
        }
        Ok(page)
    }
}

impl PackageInfo {
    /// The compact entry for one installed version, read from its manifest.
    /// Errors with `content_not_found` when the version is not installed.
    pub fn installed(packages_dir: &Path, id: &str, version: &str) -> Result<Self> {
        let dir = packages_dir.join(id).join(version);
        if !dir.join(MANIFEST_NAME).is_file() {
            return Err(error(
                "content_not_found",
                format!("{id}@{version} is not installed"),
            ));
        }
        installed_entry(packages_dir, id, version, &dir)
    }
}

fn scan(packages_dir: &Path) -> Result<(Vec<PackageInfo>, Vec<ContentError>)> {
    let mut entries = Vec::new();
    let mut diagnostics = Vec::new();
    let mut candidates = 0usize;
    for id_dir in read_dir(packages_dir)? {
        let id = id_dir.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') || !is_dir(&id_dir)? {
            continue;
        }
        for version_dir in read_dir(&id_dir.path())? {
            let version = version_dir.file_name().to_string_lossy().into_owned();
            if semver::Version::parse(&version).is_err() || !is_dir(&version_dir)? {
                continue;
            }
            candidates += 1;
            if candidates > MAX_PACKAGE_CANDIDATES {
                return Err(error(
                    "catalog_unavailable",
                    format!("Package cache exceeds {MAX_PACKAGE_CANDIDATES} versions"),
                ));
            }
            match installed_entry(packages_dir, &id, &version, &version_dir.path()) {
                Ok(entry) => entries.push(entry),
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
    }
    Ok((entries, diagnostics))
}

fn installed_entry(
    packages_dir: &Path,
    id: &str,
    version: &str,
    dir: &Path,
) -> Result<PackageInfo> {
    let manifest = crate::pkg::parse_path(dir.join(MANIFEST_NAME))
        .map_err(|e| error("invalid_content", format!("{id}@{version}: {e}")))?;
    if manifest.id != id || manifest.version != version {
        return Err(error(
            "invalid_content",
            format!(
                "{id}@{version}: manifest declares {}@{}",
                manifest.id, manifest.version
            ),
        ));
    }
    let content_ref = matches!(
        manifest.kind,
        PackageKind::Development | PackageKind::Invention
    )
    .then(|| ContentRef::Package {
        package: id.to_string(),
        version: version.to_string(),
    });
    Ok(PackageInfo {
        id: manifest.id,
        version: manifest.version,
        kind: manifest.kind,
        source: provenance(packages_dir, id, version),
        summary: truncate(manifest.description.as_deref().unwrap_or_default()),
        content_ref,
        dependencies: manifest.deps,
        module_types: Vec::new(),
    })
}

/// `bundled` when the install receipt says so; any other or missing receipt
/// is an ordinary install.
fn provenance(packages_dir: &Path, id: &str, version: &str) -> PackageSource {
    std::fs::read(receipt_path(packages_dir, id, version))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ContentReceipt>(&bytes).ok())
        .filter(|receipt| receipt.bundled)
        .map_or(PackageSource::Installed, |_| PackageSource::Bundled)
}

fn read_dir(path: &Path) -> Result<Vec<std::fs::DirEntry>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(unavailable(path, e)),
    };
    let mut entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| unavailable(path, e))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn is_dir(entry: &std::fs::DirEntry) -> Result<bool> {
    entry
        .file_type()
        .map(|kind| kind.is_dir())
        .map_err(|e| unavailable(&entry.path(), e))
}

fn unavailable(path: &Path, e: std::io::Error) -> ContentError {
    error(
        "catalog_unavailable",
        format!("cannot read package cache {}: {e}", path.display()),
    )
}

fn compare_versions(a: &PackageInfo, b: &PackageInfo) -> std::cmp::Ordering {
    match (
        semver::Version::parse(&a.version),
        semver::Version::parse(&b.version),
    ) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.version.cmp(&b.version),
    }
}

/// Binds a cursor to the filters and to the exact matching inventory, so any
/// install or removal expires it instead of skipping or repeating entries.
fn cursor_prefix(query: &PackageListQuery, matching: &[PackageInfo]) -> String {
    let mut hasher = Sha256::new();
    let filters = (query.kind, query.source, &query.id, query.detail);
    hasher.update(serde_json::to_vec(&filters).unwrap_or_default());
    for entry in matching {
        hasher.update(entry.id.as_bytes());
        hasher.update([0]);
        hasher.update(entry.version.as_bytes());
        hasher.update([0]);
    }
    let digest = crate::hex::lower_hex(&hasher.finalize());
    format!("{}:", &digest[..16])
}

fn encoded_len(value: &impl serde::Serialize) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|e| error("catalog_unavailable", e.to_string()))
}
