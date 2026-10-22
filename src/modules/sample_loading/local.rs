//! Bounded reads of local sample files, hashed so the decode cache keys on
//! the exact bytes it decoded.

/// A local sample file's bytes and their SHA-256, or `None`s for a URL
/// (fetched by `open_source`).
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_local(source: &str) -> Result<(Option<Vec<u8>>, Option<String>), String> {
    use sha2::{Digest, Sha256};
    if source.starts_with("https://") || source.starts_with("http://") {
        return Ok((None, None));
    }
    // Bounded like package audio: a regular file under the audio read cap,
    // read to exactly its opened size (so `/dev/zero` or a growing file is
    // refused rather than read without end).
    let fail = |err: crate::pkg::read_limit::ReadError| {
        format!("Failed to open sample '{}': {}", source, err)
    };
    let file =
        crate::pkg::read_limit::HashedFile::open(std::path::Path::new(source)).map_err(fail)?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.stream(|chunk| bytes.extend_from_slice(chunk))
        .map_err(fail)?;
    let digest = crate::hex::lower_hex(&Sha256::digest(&bytes));
    Ok((Some(bytes), Some(digest)))
}

/// Sample loading is unavailable on wasm32; `open_source` reports why.
#[cfg(target_arch = "wasm32")]
pub(super) fn read_local(_source: &str) -> Result<(Option<Vec<u8>>, Option<String>), String> {
    Ok((None, None))
}
