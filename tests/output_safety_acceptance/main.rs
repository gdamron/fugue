//! Acceptance tests for output safety, written from the contract alone and
//! driving only public boundaries: the DAC module, a live invention played
//! through a capturing `AudioBackend`, and the offline `RenderEngine`.
//!
//! Criteria:
//! - a NaN or +/-inf sample on either DAC input channel is written as 0.0
//!   (the mono `audio` input feeds both channels), live and offline, while
//!   finite samples are untouched (`dac_direct`, `live`, `offline`);
//! - the DAC counts replaced samples per channel, the count shows in
//!   `MasterLevels`/`MeterReading` as a running total, and is serialized
//!   only when nonzero (`dac_direct`, `live`);
//! - flush-to-zero is set inside every live callback and offline render on
//!   aarch64 and x86_64, and the calling thread is left as found
//!   (`live`, `offline`);
//! - a subnormal reaches the output as zero when the flag is set (`live`,
//!   `offline`).
//!
//! Run with `cargo test --release --test output_safety_acceptance`.

mod dac_direct;
mod live;
mod offline;
mod probe;

pub use fugue::{DacModule, Module, SinkModule};
pub use serde_json::{json, Value};

/// Whether this target is one the contract requires flush-to-zero on.
pub const FTZ_TARGET: bool = cfg!(any(target_arch = "aarch64", target_arch = "x86_64"));

/// The smallest positive subnormal times a few: nonzero, below the normal range.
pub fn subnormal() -> f32 {
    f32::from_bits(0x0000_0400)
}
