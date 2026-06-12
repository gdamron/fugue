//! Feature-gated WebAssembly module plugin support.
//!
//! This module hosts Fugue module components through Wasmtime's Component
//! Model. It is native-only and intentionally absent from browser wasm builds.

#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
pub mod wasm;
