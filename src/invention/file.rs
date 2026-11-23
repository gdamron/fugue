//! Reading an invention document from a file.
//!
//! A document file holds the invention JSON either as is or gzip-compressed
//! (RFC 1952): the JSON document, gzipped, and nothing else, so `gunzip -c`
//! prints it. Readers tell the two apart by content, never by name: a file
//! that starts with the gzip magic bytes `1f 8b` is decompressed (JSON cannot
//! start with them), and any other file is read as JSON, whatever its
//! extension.
//!
//! The document a file decompresses to is held to the same 16 MiB limit as a
//! plain one ([`DOCUMENT_READ_LIMIT`]), so a small file that expands without
//! end is refused once it passes the limit, not after.
//!
//! Fugue writes documents as plain JSON on disk; a compressed one comes from
//! elsewhere, such as a host that hands a person a piece to keep.
//!
//! [`DOCUMENT_READ_LIMIT`]: crate::pkg::read_limit::DOCUMENT_READ_LIMIT
//!
//! Reading needs a file system and the gzip decoder, which the wasm engine
//! does without: there, only [`is_compressed`] and [`text`] are built.

use crate::pkg::read_limit::{self, ReadError, ReadKind};
#[cfg(not(target_arch = "wasm32"))]
use flate2::read::MultiGzDecoder;
#[cfg(not(target_arch = "wasm32"))]
use std::borrow::Cow;
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Read;
use std::path::{Path, PathBuf};

/// Whether `bytes` are gzip-compressed: they start with its magic bytes.
pub fn is_compressed(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// Why a document file could not be read. Each message names the file,
/// except an I/O error's, which is the system's own.
#[derive(Debug)]
pub enum DocumentFileError {
    /// The file could not be read, or is over the limit on disk.
    Read(ReadError),
    /// The file decompresses to more than the document limit.
    ExpandsTooFar { path: PathBuf, limit: u64 },
    /// The file starts as gzip but is not a whole, valid gzip stream.
    Corrupt { path: PathBuf, reason: String },
    /// The document is not UTF-8 text.
    NotText { path: PathBuf },
}

impl fmt::Display for DocumentFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentFileError::Read(e) => e.fmt(f),
            DocumentFileError::ExpandsTooFar { path, limit } => write!(
                f,
                "{} is compressed and expands past the {} limit on documents; {}",
                path.display(),
                read_limit::human_bytes(*limit),
                ReadKind::Document.remedy()
            ),
            DocumentFileError::Corrupt { path, reason } => write!(
                f,
                "{} starts as a compressed (gzip) document but cannot be decompressed: {reason}",
                path.display()
            ),
            DocumentFileError::NotText { path } => {
                write!(f, "{} is not a UTF-8 text document", path.display())
            }
        }
    }
}

impl std::error::Error for DocumentFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DocumentFileError::Read(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ReadError> for DocumentFileError {
    fn from(e: ReadError) -> Self {
        DocumentFileError::Read(e)
    }
}

/// The document `bytes` read from `path` hold: decompressed if they are
/// gzip, borrowed as they are if not. Refused past the document limit.
#[cfg(not(target_arch = "wasm32"))]
pub fn decode<'a>(path: &Path, bytes: &'a [u8]) -> Result<Cow<'a, [u8]>, DocumentFileError> {
    if !is_compressed(bytes) {
        return Ok(Cow::Borrowed(bytes));
    }
    let limit = ReadKind::Document.limit();
    let mut document = Vec::new();
    MultiGzDecoder::new(bytes)
        .take(limit + 1)
        .read_to_end(&mut document)
        .map_err(|e| DocumentFileError::Corrupt {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    if document.len() as u64 > limit {
        return Err(DocumentFileError::ExpandsTooFar {
            path: path.to_path_buf(),
            limit,
        });
    }
    Ok(Cow::Owned(document))
}

/// A document file read from disk.
#[cfg(not(target_arch = "wasm32"))]
pub struct DocumentFile {
    /// The bytes on disk, which a lockfile or package integrity hashes.
    pub bytes: Vec<u8>,
    /// The document they hold, as text.
    pub text: String,
}

/// Reads the document file at `path`: plain or compressed JSON, each held
/// to the document limit.
#[cfg(not(target_arch = "wasm32"))]
pub fn read(path: &Path) -> Result<DocumentFile, DocumentFileError> {
    let bytes = read_limit::read_document(path)?;
    let text = text(path, decode(path, &bytes)?.into_owned())?;
    Ok(DocumentFile { bytes, text })
}

/// `document` as UTF-8 text.
pub fn text(path: &Path, document: Vec<u8>) -> Result<String, DocumentFileError> {
    String::from_utf8(document).map_err(|_| DocumentFileError::NotText {
        path: path.to_path_buf(),
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "file_tests.rs"]
mod tests;
