//! Physical models of ringing bodies: a plucked string and a bank of modes,
//! shared by the resonator and the macro oscillator.

use crate::dsp::exp2;

mod modal;
mod string;

pub(crate) use modal::{ModalSettings, ModalVoice, MAX_MODES};
pub(crate) use string::{KarplusString, StringSettings};

/// The time the fundamental takes to fall 60 dB at `damping` 0, and how
/// many octaves of time the damping curve spans.
const SHORTEST_T60: f32 = 0.07;
const T60_OCTAVES: f32 = 8.0;

/// The time the fundamental takes to fall 60 dB at `damping` (0 to 1):
/// 0.07 s at 0, about 4.5 s at 0.5 and 18 s at 1, rising fastest at the
/// short end.
pub(crate) fn t60_seconds(damping: f32) -> f32 {
    let d = damping.clamp(0.0, 1.0);
    SHORTEST_T60 * exp2(T60_OCTAVES * d * (2.0 - d))
}
