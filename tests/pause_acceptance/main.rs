//! Acceptance tests for pausing and resuming a loaded piece, written from the
//! behavior spec alone and driving only public boundaries: the offline
//! `RenderEngine`, and a running invention played through a capturing
//! `AudioBackend`, one 64-frame callback at a time (48 kHz).
//!
//! Criteria:
//! - a pause fades the output out over 10 ms and then holds exactly 0.0; a
//!   resume fades it back in; no step anywhere across pause, silence and
//!   resume exceeds the tone's own step plus the fade's (`fade`);
//! - pausing a paused piece, and resuming a playing one, change nothing;
//!   `RuntimeStatus` reports `paused` and `playing()` (`fade`);
//! - clocks hold while paused and continue from their beat; timed writes
//!   (on beats, bars or samples) wait through a pause (`timing`);
//! - control writes, connects, disconnects and a reload made while paused
//!   apply and are heard after resume; unloading while paused stops, and a
//!   new load plays (`edits`).
//!
//! Run with `cargo test --release --test pause_acceptance`.

mod edits;
mod fade;
mod harness;
mod timing;

pub use harness::*;
pub use serde_json::{json, Value};
