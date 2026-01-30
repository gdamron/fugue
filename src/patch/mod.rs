//! Patch system for building and running modular synthesis setups.
//!
//! This module provides infrastructure for creating modular synthesis patches
//! from JSON definitions, building the signal processing graph, and managing
//! runtime execution.
//!
//! # Components
//!
//! - [`format`] - JSON patch format definitions
//! - [`builder`] - Patch construction and validation
//! - [`runtime`] - Patch execution and control
//! - [`graph`] - Signal processing graph (pull-based)
//! - [`handles`] - Runtime control handles

pub mod builder;
pub mod format;
pub mod graph;
pub mod handles;
pub mod runtime;

pub use builder::PatchBuilder;
pub use format::{Connection, ModuleSpec, Patch, TimeSignature};
pub use handles::PatchHandles;
pub use runtime::{PatchRuntime, RunningPatch};
