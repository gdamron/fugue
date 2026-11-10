//! Library builds of the `fugue` crate for other languages.
//!
//! Natively this links the C interface (`fugue::ffi`, declared in
//! `include/fugue.h`) into a static and a dynamic library. On wasm32 it links
//! the wasm-bindgen engine (`fugue::wasm`) into the web module. Every exported
//! symbol is defined in `fugue`; this crate only gives them library outputs.
//!
//! The re-exports are what link `fugue` in: rustc never loads a dependency
//! nothing names, and then none of its exported symbols reach the libraries.

#[cfg(not(target_arch = "wasm32"))]
pub use fugue_core::ffi::*;

#[cfg(target_arch = "wasm32")]
pub use fugue_core::wasm::*;
