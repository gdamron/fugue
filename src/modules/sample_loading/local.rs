//! Samples read in place from a package's source checkout.
//!
//! An installed package version is immutable (and verified), and a plain local
//! path keeps the cheap path-keyed cache hit. A source checkout is edited in
//! place, so its samples are read once per load, hashed, and cached by the
//! digest of the bytes decoded: an edit then decodes afresh instead of
//! replaying the first decode cached at that path. A file in a package a
//! frozen session's lockfile governs is read the same way, and its digest
//! must be the one verified.

/// The bytes of `source` and their SHA-256 when it lies inside a source
/// checkout under the package cache; `None` otherwise (keyed by path).
///
/// The read is bounded like the package's own integrity hash (a regular file
/// under the audio read cap, read to exactly its opened size), since the whole
/// file is held in memory to hash and then decode it.
///
/// While a frozen session's lockfile governs a package `source` lies in, the
/// same read serves it, whatever the package's layout: the bytes must hash to
/// the digest verified for that file, and the ones that do are the ones
/// decoded and the digest that keys the cache, so a governed load never plays
/// bytes it did not verify, nor a buffer decoded from other bytes at the same
/// path.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_digested_sample(source: &str) -> Result<Option<(Vec<u8>, String)>, String> {
    use sha2::{Digest, Sha256};
    let lock = crate::pkg::lock::governing();
    let expected = match lock
        .as_ref()
        .and_then(|lock| lock.expected_digest(source.as_ref()))
    {
        Some(expected) => Some(expected?),
        None => None,
    };
    if expected.is_none() && !in_source_checkout(std::path::Path::new(source)) {
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
    if let (Some(lock), Some(expected)) = (&lock, expected) {
        if digest != expected {
            return Err(lock.changed(&source));
        }
    }
    Ok(Some((bytes, digest)))
}

/// Source checkouts exist only where packages are read from disk.
#[cfg(target_arch = "wasm32")]
pub(super) fn read_digested_sample(_source: &str) -> Result<Option<(Vec<u8>, String)>, String> {
    Ok(None)
}

/// Whether `path` is inside `<packages>/<id>/` where `<id>` is a source
/// checkout (`crate::pkg::layout`). Package refs resolve to paths under the
/// package cache as configured, so the prefix is compared as written: no
/// filesystem call is made for paths elsewhere, which keeps their cache hit a
/// map lookup. A route to a checkout file from outside the package cache is a
/// plain local path.
#[cfg(not(target_arch = "wasm32"))]
fn in_source_checkout(path: &std::path::Path) -> bool {
    let Ok(packages) = crate::pkg::default_packages_dir() else {
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
