//! How long the resonator rings: one damping curve for every model.

use crate::dsp::physical::t60_seconds;

/// The damping the sympathetic strings ring at, whatever the main
/// string's: they keep a halo going after it is damped.
pub(super) const SYMPATHETIC_DAMPING: f32 = 0.95;

/// How much longer than its 60 dB time a ring takes to fall below
/// −90 dBFS, with room for the limiter holding a loud strike near full
/// scale before it lets go.
const RING_PER_T60: f32 = 2.0;

/// The stated ring time: after a single strike, the output of `model` at
/// `damping` falls below −90 dBFS within this many seconds and stays there.
/// `None` for a model the resonator does not have.
pub fn ring_seconds(model: &str, damping: f32) -> Option<f32> {
    let longest = match model {
        "modal" | "string" => damping,
        "sympathetic_strings" => damping.max(SYMPATHETIC_DAMPING),
        _ => return None,
    };
    Some(RING_PER_T60 * t60_seconds(longest))
}
