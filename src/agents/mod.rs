//! Runtime workers for graph-resident `agent` modules.
//!
//! Agent modules live in the audio graph so inventions can trigger them like
//! any other module, but all expensive work happens here on non-audio threads.
//! The audio module only increments trigger/reset counters; these workers
//! observe those counters, build bounded context packets from runtime snapshots,
//! call configured backends, and optionally write validated results back through
//! normal graph controls.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub use native::AgentManager;
#[cfg(target_arch = "wasm32")]
pub use wasm::AgentManager;
