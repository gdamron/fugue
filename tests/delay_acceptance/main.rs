//! Acceptance tests for the `delay` atom and the `clock` module's `bpm`
//! output, written from the interface spec alone. They drive only the public
//! API (`ModuleRegistry`, `Module`, `RenderEngine`) and measure what is
//! heard: echo positions, bit-exactness, steps (clicks), pitch glide,
//! stability and the declared control surface.
//!
//! The `delay` type is built by name, so this suite compiles before the atom
//! exists and fails at runtime until it does.
//!
//! Spec summary: delay in samples = `time` x sample rate (division `off`) or
//! beats x 60 / bpm x sample rate; a whole number of samples is exact; the
//! `bpm` input overrides the `bpm` control; internal feedback spaces repeats
//! exactly the delay time, a loop patched outside adds one sample per pass;
//! `glide` = crossfade (20 ms) or slew (about 0.2 s); feedback to 1.1 stays
//! inside +/-1; `hold` loops one division and every pass after the first is
//! bit-identical; `clock` gains a trailing `bpm` output.
//!
//! Assumptions beyond the spec are marked "Assumption" on the test that makes
//! them. Run with `cargo test --release --test delay_acceptance`.

mod rig;

mod clock_sync;
mod controls;
mod exact_delay;
mod feedback;
mod glide;
mod hold;
mod tempo;

pub use fugue::{ControlKind, ControlValue, GraphModule, Module, ModuleRegistry, RenderEngine};
pub use serde_json::{json, Value};
