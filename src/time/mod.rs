//! Timing and tempo management for the synthesis engine.
//!
//! - [`Tempo`] - Thread-safe BPM controller
//! - [`Clock`] - Master clock generator for synchronization

mod clock;
mod tempo;

pub use clock::Clock;
pub use tempo::Tempo;
