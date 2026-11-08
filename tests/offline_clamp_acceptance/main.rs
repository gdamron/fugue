//! Acceptance tests for the offline render's output clamp, written from the
//! contract alone and driving only public boundaries: the offline
//! `RenderEngine`, a live invention played through a capturing
//! `AudioBackend`, and (behind the `ffi` feature) the C interface.
//!
//! Contract: offline output is clamped exactly as the live device clamps it.
//! A sample is limited to `[-1.0, MAX_BELOW_ONE]`, where `MAX_BELOW_ONE` is
//! the largest `f32` below 1.0, and a non-finite sample is silence. The
//! gain staging, the DAC's soft clip and the peak meter are unchanged.
//!
//! Criteria:
//! - an invention that exceeds 1.0 renders offline with every sample in
//!   [-1, 1], through `RenderEngine` and through
//!   `fugue_engine_render_interleaved` (`clamp`, `ffi`);
//! - an invention that stays within the clamp's range renders bit for bit as
//!   it did before the clamp existed (`identity`);
//! - offline output equals the live device's for the same input: the same
//!   clamp, and non-finite samples are silence and counted (`device_parity`);
//! - the peak meter still reports the pre-clamp peak, as it does live
//!   (`meter`).
//!
//! The offline engine exposes no meter, so the meter is read on the live
//! path, headless through a capturing backend.
//!
//! Run with `cargo test --release --test offline_clamp_acceptance` (add
//! `--features ffi` to include the C interface tests).

mod clamp;
mod device_parity;
#[cfg(feature = "ffi")]
mod ffi;
mod identity;
mod meter;
mod support;
