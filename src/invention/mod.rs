//! Invention system for building and running modular synthesis setups.
//!
//! This module provides infrastructure for creating modular synthesis inventions
//! from JSON definitions, building the signal processing graph, and managing
//! runtime execution.
//!
//! # Components
//!
//! - [`mod@format`] - JSON invention format definitions
//! - [`builder`] - Invention construction and validation
//! - [`runtime`] - Invention execution and control
//! - [`graph`] - Signal processing graph (pull-based)
//! - [`handles`] - Runtime control handles

pub(crate) mod audio_assets;
pub(crate) mod authored_document;
pub mod builder;
pub mod core_version;
pub(crate) mod declared;
pub mod development;
// Only tests call the candidate step until the runtime's atomic apply path
// is built on it; remove this allowance then.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod edits;
pub mod format;
pub mod graph;
pub mod handles;
pub mod layout;
pub(crate) mod load_warnings;
mod main_clock;
#[cfg(test)]
pub(crate) mod manual_backend;
pub mod notes;
pub mod orchestration;
pub(crate) mod publish;
pub mod reload;
pub mod render;
pub mod request_log;
pub mod rpc_snapshot;
pub mod runtime;
pub mod save;
pub mod score;
pub(crate) mod seeding;
pub mod state;
pub mod taps;
pub(crate) mod timelines;

pub use builder::InventionBuilder;
pub use core_version::CoreVersion;
pub use format::{
    AssetSpec, Connection, ControlRange, DevelopmentControl, DevelopmentInput, DevelopmentOutput,
    DevelopmentSpec, Invention, ModuleSpec, TimeSignature,
};
pub use graph::{feedback_chunks, set_feedback_chunks, MasterLevels, FEEDBACK_CHUNKS_ENV};
pub use handles::InventionHandles;
pub use layout::{Layout, Position};
pub use notes::{NoteChange, NoteChanges, NoteTime, Playhead};
pub use orchestration::{OrchestrationRuntime, RuntimeController, RuntimeSnapshot};
pub use reload::{DevelopmentDefinitions, ReloadError, ReloadReport};
pub use render::{CodeModuleRuntimeInfo, RenderEngine};
pub use request_log::{
    LoggedBeat, LoggedConnection, LoggedEdit, LoggedEditOutcome, LoggedLatch, LoggedModule,
    LoggedRequest, LoggedSetting, LoggedValue, LoggedWhen, RequestLog, REQUEST_LOG_CAPACITY,
    TIMED_EDIT_LOG_CAPACITY,
};
pub use rpc_snapshot::ControlOverride;
pub use runtime::{ControlAcks, GraphCommandError, InventionRuntime, RunningInvention};
pub use score::{validate_score, Score, SCORE_SCHEMA_V1};
pub use state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};
pub use taps::{ChannelBins, OutputBins, OutputTap, MAX_OUTPUT_TAPS, MAX_TAP_BINS};
