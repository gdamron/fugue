//! Acceptance tests for removing one of several cables into an input
//! mid-note, written from the behavior spec alone and driving only public
//! boundaries: the offline `RenderEngine` and a running invention played
//! through a capturing `AudioBackend`, one 64-frame callback at a time.
//!
//! An input sums every cable patched into it. Unpatching all of them already
//! glides the input to its resting value over a 10 ms release (480 frames at
//! 48 kHz); these tests hold the same for removing only some of them, so the
//! input glides from the old sum to the new one.
//!
//! Criteria (48 kHz):
//! - two cables summed into one input: disconnecting one of them mid-note
//!   does not step, measured as the full-unpatch tests do (`fan_in`);
//! - removing one route of a fan-in, by removing its source module or by a
//!   live reload that drops it, does not step either (`fan_in`);
//! - both hold for at least three module types, with no per-module
//!   behavior: the same cases run against a vca `level` and `audio`, an
//!   oscillator `amplitude_mod` and `frequency`, a filter `cutoff_mod`, and
//!   a dac `audio` input (`fan_in`);
//! - the glide lasts the release, settles exactly on the remaining sum and
//!   never overshoots; a cable patched back in mid-glide returns the sum;
//!   several cables removed one after another, and a last cable removed
//!   after the others, glide too (`glide`);
//! - a full unpatch is sample for sample what it was before the partial
//!   release existed (`pinned`).
//!
//! Run with `cargo test --release --test partial_unpatch_acceptance`.

#![allow(dead_code)]

mod fan_in;
mod glide;
#[path = "../live_smoothing_acceptance/harness.rs"]
mod harness;
mod pinned;
mod sources;

pub use harness::*;
pub use serde_json::{json, Value};
pub use sources::*;

/// Frames in the 10 ms release.
pub const RELEASE: usize = 480;
