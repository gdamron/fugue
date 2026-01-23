//! Core module traits and signal routing primitives.
//!
//! This module provides the fundamental abstractions for building synthesis graphs:
//! - [`Module`] - Base trait for all audio processing components
//! - [`Generator`] - Modules that produce signals (oscillators, clocks)
//! - [`Processor`] - Modules that transform signals (filters, effects)
//! - [`Connect`] - Trait for chaining modules together
//! - [`Connection`] - Signal buffer between modules

mod connection;
mod traits;

pub use connection::Connection;
pub use traits::{Connect, ConnectedProcessor, Generator, Module, Processor};
