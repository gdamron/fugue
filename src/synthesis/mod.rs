//! Audio synthesis components for processing and combining signals.
//!
//! - [`Adsr`] - ADSR envelope generator (type-based)
//! - [`ModularAdsr`] - ADSR envelope with named ports (new modular system)
//! - [`Vca`] - Voltage Controlled Amplifier with named ports (new modular system)
//! - [`Mixer`] - Combines multiple audio signals
//! - [`Filter`] - Low-pass filter for tonal shaping
//! - [`Voice`] - Converts note signals to audio

mod adsr;
mod filter;
mod mixer;
mod modular_adsr;
mod vca;
mod voice;

pub use adsr::{Adsr, AdsrInput};
pub use filter::Filter;
pub use mixer::Mixer;
pub use modular_adsr::ModularAdsr;
pub use vca::Vca;
pub use voice::Voice;
