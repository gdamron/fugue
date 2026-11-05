//! Contour: one shaped ramp that works as an envelope, an LFO or an audio
//! oscillator, with four related outputs.

// The pieces land before the module that plays them.
#![allow(dead_code)]

mod shaper;
mod tables;

/// How the ramp runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RampMode {
    /// Attack then decay, once per trigger.
    Ad,
    /// Cycles continuously.
    Looping,
    /// Attack while the gate is high, release when it falls.
    Ar,
}

/// The speed the ramp is tuned for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Range {
    /// Envelopes and LFOs: asymmetric attack-decay curves.
    Control,
    /// Oscillators: band-limited, with symmetric curves.
    Audio,
}
