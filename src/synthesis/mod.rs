//! Audio synthesis components for processing and combining signals.
//!
//! - [`Adsr`] - ADSR envelope generator
//! - [`Mixer`] - Combines multiple audio signals
//! - [`Filter`] - Low-pass filter for tonal shaping
//! - [`Voice`] - Converts note signals to audio

mod adsr;
mod filter;
mod mixer;
mod voice;

pub use adsr::{Adsr, AdsrInput};
pub use filter::Filter;
pub use mixer::Mixer;
pub use voice::Voice;
