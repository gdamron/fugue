//! Platform-specific orchestration script hosts.
//!
//! Native builds embed a JavaScript runtime and execute `code` modules
//! internally. wasm builds expose the graph orchestration surface and expect the
//! surrounding JS host to run the scripts.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub use native::ScriptManager;
#[cfg(target_arch = "wasm32")]
pub use wasm::ScriptManager;
