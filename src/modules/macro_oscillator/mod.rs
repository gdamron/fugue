//! Macro oscillator: one voice, many synthesis models, played with three
//! macro controls.
//!
//! This layer holds the engines and the oscillators they are built from;
//! the module that plays them lands with the voice.

// Nothing plays the engines until the module lands.
#![allow(dead_code)]

mod engine;
mod engines;
mod oscillators;
