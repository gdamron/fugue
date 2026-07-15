//! Invention system for building and running modular synthesis setups.
//!
//! This module provides infrastructure for creating modular synthesis inventions
//! from JSON definitions, building the signal processing graph, and managing
//! runtime execution.
//!
//! # Components
//!
//! - [`format`] - JSON invention format definitions
//! - [`builder`] - Invention construction and validation
//! - [`runtime`] - Invention execution and control
//! - [`graph`] - Signal processing graph (pull-based)
//! - [`handles`] - Runtime control handles

pub mod builder;
pub mod development;
pub mod format;
pub mod graph;
pub mod handles;
pub mod orchestration;
pub mod reload;
pub mod render;
pub mod rpc_snapshot;
pub mod runtime;
pub mod save;
pub mod score;
pub mod state;

pub use builder::InventionBuilder;
pub use format::{
    AssetSpec, Connection, DevelopmentControl, DevelopmentInput, DevelopmentOutput,
    DevelopmentSpec, Invention, ModuleSpec, TimeSignature,
};
pub use handles::InventionHandles;
pub use orchestration::{OrchestrationRuntime, RuntimeController, RuntimeSnapshot};
pub use reload::{DevelopmentDefinitions, ReloadError, ReloadReport};
pub use render::{CodeModuleRuntimeInfo, RenderEngine};
pub use rpc_snapshot::ControlOverride;
pub use runtime::{GraphCommandError, InventionRuntime, RunningInvention};
pub use score::{validate_score, Score, SCORE_SCHEMA_V1};
pub use state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};
