//! Acceptance tests for click-free graph edits, written from the behaviour
//! spec alone and driving only public boundaries: the offline
//! `RenderEngine` and a running invention played through a capturing
//! `AudioBackend`, one 64-frame callback at a time.
//!
//! Criteria (48 kHz):
//! - disconnecting (or removing the source of) a connection into a module's
//!   modulation input mid-note changes the parameter smoothly: in a held
//!   tone the largest sample-to-sample step over the 20 ms after the edit is
//!   at most 1.5x the steady-state step, and the parameter is at its
//!   resting value (what the input reads unconnected) within 15 ms. This
//!   holds offline and live, and for a live reload that drops the
//!   connection (`unpatch`);
//! - an audio unpatch does not click, a connection patched back in plays its
//!   source again, and a connected path gains no latency (`connected`);
//! - a development instance whose config sets starting values plays them in
//!   its first audio block, with no glide from the inner modules' defaults
//!   (`development_start`).
//!
//! Run with `cargo test --release --test live_smoothing_acceptance`.

mod connected;
mod development_start;
mod harness;
mod unpatch;

pub use harness::*;
pub use serde_json::{json, Value};
