//! Resonator: tuned, ringing bodies excited by any sound.
//!
//! The ported DSP the module is built on: the modal bank, the voices that
//! deal strums to it, and the output limiter.

// The module that plays these lands on top of them.
#[allow(dead_code)]
mod limiter;
#[allow(dead_code)]
mod modal;
#[allow(dead_code)]
mod tuning;
#[allow(dead_code)]
mod voices;

/// The voices render in blocks of this many frames, whatever the host's
/// block size: 0.5 ms at 48 kHz.
pub(super) const ENGINE_BLOCK: usize = 24;
