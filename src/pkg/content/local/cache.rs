//! Process-wide verification cache for catalog refreshes (FUG-279).
//!
//! Listing and describing content would otherwise re-hash every package and
//! workspace audio file on every call, so the cost grew with library bytes.
//! Digests are reused while a file's metadata stamp (size, mtime and, on Unix,
//! inode and ctime) is unchanged. A package's stamp covers every file
//! `compute_integrity` hashes, so an added, removed or rewritten file misses.
//!
//! Imports never read from the cache: [`Hashing::Full`] re-hashes the whole
//! closure, which is what lets `load_development` detect changed content and
//! fail with `stale_reference`. Full hashes still refresh the cache.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// Whether a closure walk may reuse cached digests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Hashing {
    /// Reuse digests whose stamp is unchanged (list and detail).
    Cached,
    /// Re-hash every byte (imports). The default, so a new caller is never
    /// silently weaker than before.
    #[default]
    Full,
}

/// At most this many digests are kept, evicting the least recently used. An
/// entry is a path and two short hashes, so the cache stays within a few MiB.
const MAX_ENTRIES: usize = 16_384;

/// A file modified this recently could change again within the filesystem's
/// timestamp granularity without changing its stamp, so its digest is not
/// cached until it settles (the same "racy" window git guards against).
const SETTLE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    File,
    Package,
}

struct Entry {
    stamp: [u8; 32],
    digest: String,
    used: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<(Kind, PathBuf), Entry>,
    clock: u64,
}

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    // A panic while holding the lock cannot leave an entry half-written, so a
    // poisoned cache is still consistent.
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A digest of file metadata, plus whether every file in it has settled.
struct Stamp(Sha256, bool);

impl Stamp {
    fn new() -> Self {
        Self(Sha256::new(), true)
    }

    fn add(&mut self, metadata: &Metadata, now: SystemTime) {
        let modified = metadata.modified().ok();
        let since = |t: SystemTime| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        self.0.update(metadata.len().to_le_bytes());
        match modified {
            Some(t) => {
                self.0.update(since(t).as_nanos().to_le_bytes());
                // Unknown or future mtimes (clock skew) never settle.
                self.1 &= now.duration_since(t).is_ok_and(|age| age >= SETTLE);
            }
            None => self.1 = false,
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.0.update(metadata.dev().to_le_bytes());
            self.0.update(metadata.ino().to_le_bytes());
            self.0.update(metadata.ctime().to_le_bytes());
            self.0.update(metadata.ctime_nsec().to_le_bytes());
        }
    }

    fn finish(self) -> Option<[u8; 32]> {
        self.1.then(|| self.0.finalize().into())
    }
}

impl Cache {
    fn get(&mut self, key: &(Kind, PathBuf), stamp: &[u8; 32]) -> Option<String> {
        self.clock += 1;
        let entry = self.entries.get_mut(key).filter(|e| e.stamp == *stamp)?;
        entry.used = self.clock;
        Some(entry.digest.clone())
    }

    /// Insert, evicting the least recently used entry beyond `capacity`.
    fn insert(&mut self, key: (Kind, PathBuf), stamp: [u8; 32], digest: &str, capacity: usize) {
        self.clock += 1;
        if self.entries.len() >= capacity && !self.entries.contains_key(&key) {
            let oldest = self.entries.iter().min_by_key(|(_, e)| e.used);
            if let Some(oldest) = oldest.map(|(key, _)| key.clone()) {
                self.entries.remove(&oldest);
            }
        }
        let used = self.clock;
        let digest = digest.into();
        self.entries.insert(
            key,
            Entry {
                stamp,
                digest,
                used,
            },
        );
    }
}

/// The hash is computed outside the lock, so concurrent calls never wait on
/// each other's hashing; two calls that both miss simply hash twice.
fn cached<E>(
    kind: Kind,
    path: &Path,
    stamp: Option<[u8; 32]>,
    hashing: Hashing,
    compute: impl FnOnce() -> Result<String, E>,
) -> Result<String, E> {
    if let (Hashing::Cached, Some(stamp)) = (hashing, &stamp) {
        if let Some(digest) = cache().get(&(kind, path.to_path_buf()), stamp) {
            return Ok(digest);
        }
    }
    // A file that changes while it is hashed gets a new stamp, so the entry
    // stored under the stamp taken before hashing can never match it again.
    let digest = compute()?;
    #[cfg(test)]
    test_counter::HASHES.with(|h| h.set(h.get() + 1));
    if let Some(stamp) = stamp {
        cache().insert((kind, path.to_path_buf()), stamp, &digest, MAX_ENTRIES);
    }
    Ok(digest)
}

/// The digest of one file whose metadata was captured when it was opened.
pub(super) fn file_digest<E>(
    path: &Path,
    metadata: &Metadata,
    hashing: Hashing,
    compute: impl FnOnce() -> Result<String, E>,
) -> Result<String, E> {
    let mut stamp = Stamp::new();
    stamp.add(metadata, SystemTime::now());
    cached(Kind::File, path, stamp.finish(), hashing, compute)
}

/// A package's `compute_integrity`, reused while every file it hashes is unchanged.
pub(super) fn package_integrity<E>(
    root: &Path,
    hashing: Hashing,
    compute: impl FnOnce() -> Result<String, E>,
) -> Result<String, E> {
    // On a walk error, hash without caching so the failure keeps its usual diagnostics.
    let stamp = package_stamp(root).ok().flatten();
    cached(Kind::Package, root, stamp, hashing, compute)
}

fn package_stamp(root: &Path) -> Result<Option<[u8; 32]>, Box<dyn std::error::Error>> {
    let now = SystemTime::now();
    let mut stamp = Stamp::new();
    for rel in crate::pkg::lock::integrity_files(root)? {
        stamp.0.update((rel.len() as u64).to_le_bytes());
        stamp.0.update(rel.as_bytes());
        stamp.add(&std::fs::metadata(root.join(&rel))?, now);
    }
    Ok(stamp.finish())
}

/// Counts digests actually computed on this thread, so tests can show reuse.
#[cfg(test)]
pub(super) mod test_counter {
    use std::cell::Cell;

    thread_local! {
        pub(super) static HASHES: Cell<u64> = const { Cell::new(0) };
    }

    /// Digests computed on this thread while running `body`.
    pub(in crate::pkg::content::local) fn hashes_during(body: impl FnOnce()) -> u64 {
        let before = HASHES.with(Cell::get);
        body();
        HASHES.with(Cell::get) - before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_keeps_the_cache_within_capacity_and_drops_the_least_recent() {
        let mut cache = Cache::default();
        let key = |name: &str| (Kind::File, PathBuf::from(name));
        cache.insert(key("a"), [1; 32], "sha256:a", 2);
        cache.insert(key("b"), [2; 32], "sha256:b", 2);
        assert_eq!(cache.get(&key("a"), &[1; 32]).as_deref(), Some("sha256:a"));
        cache.insert(key("c"), [3; 32], "sha256:c", 2);
        assert_eq!(cache.entries.len(), 2);
        assert!(
            cache.get(&key("b"), &[2; 32]).is_none(),
            "b was least recent"
        );
        assert!(cache.get(&key("a"), &[1; 32]).is_some());
        // A changed stamp misses rather than returning the old digest.
        assert!(cache.get(&key("c"), &[4; 32]).is_none());
    }
}
