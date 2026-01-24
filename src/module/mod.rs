//! Core module traits and signal routing primitives.
//!
//! This module provides the fundamental abstractions for building synthesis graphs:
//! - [`Module`] - Base trait for all audio processing components
//! - [`Generator`] - Modules that produce signals (oscillators, clocks)
//! - [`Processor`] - Modules that transform signals (filters, effects)
//! - [`Connect`] - Trait for chaining modules together
//! - [`Connection`] - Signal buffer between modules
//! - [`ModularModule`] - Named port system for flexible signal routing (new)

mod connection;
mod modular;
mod traits;

pub use connection::Connection;
pub use modular::{validate_port, ModularModule};
pub use traits::{Connect, ConnectedProcessor, Generator, Module, Processor};
