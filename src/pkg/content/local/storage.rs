//! Bounded filesystem access, checksums and installed-package receipt storage.

use super::*;
use sha2::{Digest, Sha256};
use std::fs;

const FILE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn canonical(path: &Path, code: &str) -> Result<PathBuf> {
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
pub(super) fn contained(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = canonical(root, "content_not_found")?;
    let path = canonical(path, "dependency_not_found")?;
    if !path.starts_with(root) {
        return Err(err("outside_root", "Content escapes its declaring root"));
    }
    Ok(path)
}
pub(super) fn read_file(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path).map_err(io_error)?.len() > FILE_BYTES {
        return Err(err("invalid_content", "Content file exceeds 16 MiB"));
    }
    fs::read(path).map_err(io_error)
}
pub(super) fn read_lock(path: &Path) -> Result<Option<Lockfile>> {
    match fs::read(path) {
        Ok(bytes) => Lockfile::from_slice(&bytes)
            .map(Some)
            .map_err(|e| err("invalid_content", e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}
pub(super) fn directory(path: &Path) -> Result<Vec<fs::DirEntry>> {
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
pub(super) fn workspace_refs(
    root: &Path,
    path: &Path,
    out: &mut Vec<ContentRef>,
    depth: usize,
) -> Result<()> {
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
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
pub(super) fn revision_of(pairs: &BTreeMap<String, String>) -> Result<String> {
    Ok(hash(
        &serde_json::to_vec(&pairs.iter().collect::<Vec<_>>())
            .map_err(|e| err("invalid_content", e))?,
    ))
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
pub(super) fn read_receipt(
    packages: &Path,
    id: &str,
    version: &str,
) -> Result<Option<ContentReceipt>> {
    match fs::read(receipt_path(packages, id, version)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| err("invalid_content", e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}
pub(super) fn package_read_guard(packages: &Path) -> Result<Option<fs::File>> {
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
