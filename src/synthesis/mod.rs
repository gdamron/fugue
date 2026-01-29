//! Audio synthesis components for processing and combining signals.
//!
//! - [`ModularAdsr`] - ADSR envelope with named ports
//! - [`Vca`] - Voltage Controlled Amplifier with named ports

mod modular_adsr;
mod vca;

pub use modular_adsr::ModularAdsr;
pub use vca::Vca;
