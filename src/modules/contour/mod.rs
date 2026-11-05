//! Contour: one shaped ramp that works as an envelope, an LFO or an audio
//! oscillator, with four related outputs.

// The pieces land before the module that plays them.
#![allow(dead_code)]

mod ramp;
mod generator;
mod ratios;
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

/// What the four outputs carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// The shaped ramp, the raw ramp, and end-of-attack and end-of-cycle
    /// gates.
    Envelope,
    /// One ramp panned across the outputs.
    Amplitude,
    /// Copies offset in phase (or, for envelopes, in slope).
    Phase,
    /// Copies at related frequencies.
    Ratio,
}
