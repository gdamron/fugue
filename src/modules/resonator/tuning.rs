//! How long the resonator rings: one damping curve for every model.

use crate::dsp::exp2;

/// The time the fundamental takes to fall 60 dB at `damping` 0, and how
/// many octaves of time the damping curve spans.
const SHORTEST_T60: f32 = 0.07;
const T60_OCTAVES: f32 = 8.0;

/// How much longer than its 60 dB time a ring takes to fall below
/// −90 dBFS, with room for the limiter holding a loud strike near full
/// scale before it lets go.
const RING_PER_T60: f32 = 2.0;

/// The time the fundamental takes to fall 60 dB at `damping` (0 to 1):
/// 0.07 s at 0, about 4.5 s at 0.5 and 18 s at 1, rising fastest at the
/// short end.
pub(crate) fn t60_seconds(damping: f32) -> f32 {
    let d = damping.clamp(0.0, 1.0);
    SHORTEST_T60 * exp2(T60_OCTAVES * d * (2.0 - d))
}

/// The stated ring time: after a single strike, the output of `model` at
/// `damping` falls below −90 dBFS within this many seconds and stays there.
/// `None` for a model the resonator does not have.
pub fn ring_seconds(model: &str, damping: f32) -> Option<f32> {
    let longest = match model {
        "modal" => damping,
        _ => return None,
    };
    Some(RING_PER_T60 * t60_seconds(longest))
}
