//! Per-file read limits shared by content discovery and package integrity.
//!
//! Two kinds of file are read while discovering or verifying content:
//!
//! - **Documents** (inventions, developments, JSON/text assets) are parsed, so
//!   they are read fully into memory and capped at [`DOCUMENT_READ_LIMIT`].
//! - **Hashed files** (audio assets, and every file an installed package's
//!   integrity covers) are only hashed. They are streamed through the hasher
//!   in fixed-size chunks, never held in memory, and capped at
//!   [`AUDIO_ASSET_READ_LIMIT`].
//!
//! The workspace (local path) and package (`pkg:` / [`compute_integrity`])
//! paths apply the same rule to the same bytes. See `CONTENT_IDENTITY.md`,
//! "File read limits".
//!
//! [`compute_integrity`]: crate::pkg::compute_integrity

use std::fmt;
use std::path::PathBuf;

/// Largest document discovery parses: 16 MiB.
pub const DOCUMENT_READ_LIMIT: u64 = 16 * 1024 * 1024;

/// Largest file discovery and package integrity will hash: 1 GiB.
///
/// About 100 minutes of 44.1 kHz 16-bit stereo, or 30 minutes of 96 kHz
/// 24-bit stereo. Hashing streams, so memory stays flat; the cap bounds the
/// time one file can cost (a second or two at native SHA-256 speed). Larger
/// files are impractical anyway: the sample player decodes a whole file into
/// memory as `f32`, roughly doubling a 16-bit file's size.
pub const AUDIO_ASSET_READ_LIMIT: u64 = 1024 * 1024 * 1024;

/// What a file is read for, which picks its limit and the remedy we suggest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    /// Parsed in memory; capped at [`DOCUMENT_READ_LIMIT`].
    Document,
    /// Stream-hashed only; capped at [`AUDIO_ASSET_READ_LIMIT`].
    Hashed,
}

impl ReadKind {
    /// The byte limit that applies to this kind of read.
    pub fn limit(self) -> u64 {
        #[cfg(test)]
        if let Some(limit) = test_override::get(self) {
            return limit;
        }
        match self {
            ReadKind::Document => DOCUMENT_READ_LIMIT,
            ReadKind::Hashed => AUDIO_ASSET_READ_LIMIT,
        }
    }

    fn remedy(self) -> &'static str {
        match self {
            ReadKind::Document => {
                "split the document, or move bulky data into a separate asset file"
            }
            ReadKind::Hashed => "trim or split the sample, or re-encode it as FLAC to shrink it",
        }
    }
}

/// A file exceeded its read limit. Its message names the file, the limit and
/// the remedy, so it can be shown to a person as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTooLarge {
    pub path: PathBuf,
    pub size: u64,
    pub limit: u64,
    pub kind: ReadKind,
}

impl fmt::Display for FileTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            ReadKind::Document => "documents",
            ReadKind::Hashed => "audio assets and package files",
        };
        write!(
            f,
            "{} is {} but {what} are limited to {}; {}",
            self.path.display(),
            size_against(self.size, self.limit),
            human_bytes(self.limit),
            self.kind.remedy()
        )
    }
}

impl std::error::Error for FileTooLarge {}

/// Why a bounded read failed.
#[derive(Debug)]
pub enum ReadError {
    TooLarge(FileTooLarge),
    Io(std::io::Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::TooLarge(e) => e.fmt(f),
            ReadError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReadError::TooLarge(e) => Some(e),
            ReadError::Io(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// Find a [`FileTooLarge`] in an error chain, e.g. one returned through
/// [`compute_integrity`](crate::pkg::compute_integrity)'s boxed error.
pub fn find_too_large<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a FileTooLarge> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(found) = e.downcast_ref::<FileTooLarge>() {
            return Some(found);
        }
        current = e.source();
    }
    None
}

fn human_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    match bytes {
        b if b >= GIB && b % GIB == 0 => format!("{} GiB", b / GIB),
        b if b >= MIB && b % MIB == 0 => format!("{} MiB", b / MIB),
        b if b >= MIB => format!("{:.1} MiB", b as f64 / MIB as f64),
        b => format!("{b} bytes"),
    }
}

/// A size next to its limit: exact bytes when rounding would make them match.
fn size_against(size: u64, limit: u64) -> String {
    let (size_text, limit_text) = (human_bytes(size), human_bytes(limit));
    if size_text == limit_text || size_text == format!("{:.1} MiB", limit as f64 / 1048576.0) {
        format!("{size} bytes")
    } else {
        size_text
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod fs_ops {
    use super::*;
    use std::fs::File;
    use std::io::Read;
    use std::path::Path;

    fn too_large(path: &Path, size: u64, limit: u64, kind: ReadKind) -> ReadError {
        ReadError::TooLarge(FileTooLarge {
            path: path.to_path_buf(),
            size,
            limit,
            kind,
        })
    }

    /// Open `path` and check its size against `kind`'s limit before reading.
    fn open_bounded(path: &Path, kind: ReadKind) -> Result<(File, u64, u64), ReadError> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        let limit = kind.limit();
        if size > limit {
            return Err(too_large(path, size, limit, kind));
        }
        Ok((file, size, limit))
    }

    /// Read a whole document into memory, refusing files over its limit.
    /// A file that grows past the limit while being read is refused too.
    pub fn read_document(path: &Path) -> Result<Vec<u8>, ReadError> {
        let kind = ReadKind::Document;
        let (file, size, limit) = open_bounded(path, kind)?;
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(too_large(path, bytes.len() as u64, limit, kind));
        }
        Ok(bytes)
    }

    /// A file opened for streaming hash, already checked against
    /// [`ReadKind::Hashed`]'s limit from its metadata.
    pub struct HashedFile {
        path: PathBuf,
        file: File,
        metadata: std::fs::Metadata,
    }

    impl HashedFile {
        /// Open `path`, refusing it before any read if it is over the limit.
        pub fn open(path: &Path) -> Result<Self, ReadError> {
            let (file, _, _) = open_bounded(path, ReadKind::Hashed)?;
            let metadata = file.metadata()?;
            Ok(Self {
                path: path.to_path_buf(),
                file,
                metadata,
            })
        }

        /// Size in bytes at open time; the stream must match it exactly.
        pub fn size(&self) -> u64 {
            self.metadata.len()
        }

        /// Metadata at open time (size, mtime): the key a hash cache uses.
        pub fn metadata(&self) -> &std::fs::Metadata {
            &self.metadata
        }

        /// Feed the file through `update` in fixed-size chunks without
        /// holding it in memory. A file that changes length mid-read is an
        /// error rather than a silently different hash.
        pub fn stream(mut self, mut update: impl FnMut(&[u8])) -> Result<u64, ReadError> {
            let size = self.size();
            let mut buffer = vec![0u8; 64 * 1024];
            let mut seen = 0u64;
            loop {
                let read = match self.file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                };
                seen += read as u64;
                if seen > size {
                    break;
                }
                update(&buffer[..read]);
            }
            if seen != size {
                return Err(ReadError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{} changed size while it was being hashed",
                        self.path.display()
                    ),
                )));
            }
            Ok(size)
        }

        /// SHA-256 of the file's exact bytes as `sha256:<hex>`, streamed.
        pub fn sha256(self) -> Result<String, ReadError> {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            self.stream(|chunk| hasher.update(chunk))?;
            Ok(format!(
                "sha256:{}",
                crate::hex::lower_hex(&hasher.finalize())
            ))
        }
    }

    /// SHA-256 of a hashed file's exact bytes as `sha256:<hex>`, streamed
    /// under [`ReadKind::Hashed`]'s limit.
    pub fn hash_file(path: &Path) -> Result<String, ReadError> {
        HashedFile::open(path)?.sha256()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs_ops::{hash_file, read_document, HashedFile};

/// Test-only per-thread limits, so tests can cross a limit with small files.
#[cfg(test)]
pub(crate) mod test_override {
    use super::ReadKind;
    use std::cell::Cell;

    thread_local! {
        static LIMITS: Cell<(Option<u64>, Option<u64>)> = const { Cell::new((None, None)) };
    }

    pub(super) fn get(kind: ReadKind) -> Option<u64> {
        let (document, hashed) = LIMITS.with(Cell::get);
        match kind {
            ReadKind::Document => document,
            ReadKind::Hashed => hashed,
        }
    }

    /// Run `body` with the hashed-file limit set to `limit` on this thread.
    pub(crate) fn with_hashed_limit<T>(limit: u64, body: impl FnOnce() -> T) -> T {
        let previous = LIMITS.with(Cell::get);
        LIMITS.with(|l| l.set((previous.0, Some(limit))));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        LIMITS.with(|l| l.set(previous));
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;

    #[test]
    fn streamed_hash_matches_in_memory_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.wav");
        // Several chunks plus a partial one.
        let bytes: Vec<u8> = (0..200_003u32).map(|i| (i * 31 % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            hash_file(&path).unwrap(),
            format!("sha256:{}", crate::hex::lower_hex(&Sha256::digest(&bytes)))
        );
    }

    #[test]
    fn audio_over_the_document_limit_is_hashed_not_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.wav");
        // Sparse: costs no disk, reads as zeros.
        fs::File::create(&path)
            .unwrap()
            .set_len(DOCUMENT_READ_LIMIT + 1)
            .unwrap();
        assert!(matches!(
            read_document(&path),
            Err(ReadError::TooLarge(FileTooLarge {
                kind: ReadKind::Document,
                ..
            }))
        ));
        assert!(hash_file(&path).unwrap().starts_with("sha256:"));
    }

    #[test]
    fn over_cap_audio_is_refused_before_reading_and_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hour-long-take.wav");
        // Sparse, and refused from metadata alone, so this test is instant.
        fs::File::create(&path)
            .unwrap()
            .set_len(AUDIO_ASSET_READ_LIMIT + 1)
            .unwrap();
        let ReadError::TooLarge(error) = hash_file(&path).unwrap_err() else {
            panic!("expected FileTooLarge");
        };
        assert_eq!(error.limit, AUDIO_ASSET_READ_LIMIT);
        let message = error.to_string();
        assert!(message.contains("hour-long-take.wav"), "{message}");
        assert!(message.contains("1 GiB"), "{message}");
        assert!(message.contains("trim or split the sample"), "{message}");
    }

    #[test]
    fn too_large_is_found_through_boxed_errors() {
        let error: Box<dyn std::error::Error> = Box::new(ReadError::TooLarge(FileTooLarge {
            path: "a.wav".into(),
            size: 2,
            limit: 1,
            kind: ReadKind::Hashed,
        }));
        assert_eq!(find_too_large(error.as_ref()).unwrap().size, 2);
    }
}
