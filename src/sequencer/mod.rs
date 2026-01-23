//! Melody generation and sequencing.
//!
//! - [`MelodyGenerator`] - Generates notes from a scale with weighted random selection
//! - [`MelodyParams`] - Thread-safe parameters for melody control
//! - [`NoteSignal`] - Combined gate and frequency signal

mod melody_generator;
mod melody_params;
mod note_signal;

pub use melody_generator::MelodyGenerator;
pub use melody_params::MelodyParams;
pub use note_signal::NoteSignal;
