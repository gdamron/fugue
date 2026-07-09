//! Shared streaming backends used by sink/tap modules.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod ffmpeg;
