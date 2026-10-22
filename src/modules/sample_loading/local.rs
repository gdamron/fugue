//! Samples read in place from a package's source checkout.
//!
//! An installed package version is immutable (and verified), and a plain local
//! path keeps the cheap path-keyed cache hit. A source checkout is edited in
//! place, so its samples are read once per load, hashed, and cached by the
//! digest of the bytes decoded: an edit then decodes afresh instead of
//! replaying the first decode cached at that path.

/// The bytes of `source` and their SHA-256 when it lies inside a source
/// checkout under the package cache; `None` otherwise (keyed by path).
///
/// The read is bounded like the package's own integrity hash (a regular file
/// under the audio read cap, read to exactly its opened size), since the whole
/// file is held in memory to hash and then decode it.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_checkout_sample(source: &str) -> Result<Option<(Vec<u8>, String)>, String> {
    use sha2::{Digest, Sha256};
    if !in_source_checkout(std::path::Path::new(source)) {
        return Ok(None);
    }
    let fail = |err: crate::pkg::read_limit::ReadError| {
        format!("Failed to open sample '{}': {}", source, err)
    };
    let file =
        crate::pkg::read_limit::HashedFile::open(std::path::Path::new(source)).map_err(fail)?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.stream(|chunk| bytes.extend_from_slice(chunk))
        .map_err(fail)?;
    let digest = crate::hex::lower_hex(&Sha256::digest(&bytes));
    Ok(Some((bytes, digest)))
}

/// Source checkouts exist only where packages are read from disk.
#[cfg(target_arch = "wasm32")]
pub(super) fn read_checkout_sample(_source: &str) -> Result<Option<(Vec<u8>, String)>, String> {
    Ok(None)
}

/// Whether `path` is inside `<packages>/<id>/` where `<id>` is a source
/// checkout (`crate::pkg::layout`). URLs and paths outside the package cache
/// are not; both sides are canonicalized, so a relative or symlinked route
/// to a checkout file is recognized too.
#[cfg(not(target_arch = "wasm32"))]
fn in_source_checkout(path: &std::path::Path) -> bool {
    let Ok(packages) = crate::pkg::default_packages_dir()
        .and_then(|dir| std::fs::canonicalize(dir).map_err(|e| e.to_string()))
    else {
        return false;
    };
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    let Some(std::path::Component::Normal(id)) = path
        .strip_prefix(&packages)
        .ok()
        .and_then(|relative| relative.components().next())
    else {
        return false;
    };
    id.to_str()
        .is_some_and(|id| crate::pkg::is_source_checkout(&packages, id))
}
