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
pub mod render;
pub mod runtime;
pub mod state;

pub use builder::InventionBuilder;
pub use format::{
    Connection, DevelopmentControl, DevelopmentInput, DevelopmentOutput, DevelopmentSpec,
    Invention, ModuleSpec, TimeSignature,
};
pub use handles::InventionHandles;
pub use orchestration::{OrchestrationRuntime, RuntimeController, RuntimeSnapshot};
pub use render::{CodeModuleRuntimeInfo, RenderEngine};
pub use runtime::{GraphCommandError, InventionRuntime, RunningInvention};
pub use state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};
