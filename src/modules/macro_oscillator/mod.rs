//! Macro oscillator: one voice, many synthesis models, played with three
//! macro controls.
//!
//! This layer holds the voice (the model crossfade and the low-pass
//! gate), the engines and the oscillators they are built from; the module
//! that plays the voice lands next.

// Nothing plays the voice until the module lands.
#![allow(dead_code)]

mod engine;
mod engines;
mod lpg;
mod oscillators;
mod voice;
