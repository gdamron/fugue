//! Acceptance tests for the dynamics atoms `gain`, `follower` and
//! `gain_curve`, written from their spec alone: the issue's definition of done
//! and the pinned interface (`gain`: `audio` + `decibels_mod` in, controls
//! `decibels`, `decibels_mod_depth`, `invert`; `follower`: `audio_left`,
//! `audio_right`, `sidechain` in, `level` out, controls `attack`, `release`,
//! `sidechain_mix`, `highpass`; `gain_curve`: `level` in, `decibels` out,
//! controls `threshold`, `ratio`, `knee`). They drive only the public API
//! (`ModuleRegistry`, `GraphModule`, `Module`).
//!
//! Criteria: the static curve is within 0.1 dB of the design at threshold
//! -20, ratio 4, knee 0 and 6, from -40 to 0 dBFS; attack and release (time
//! to 63%) within 10%; `gain` at 0 dB is a bit-identical passthrough, +6.0206
//! dB is x2 within 1e-4, `decibels_mod` sums with `decibels` (times its
//! depth), `invert` flips the sign; controls are declared.
//!
//! Assumptions beyond the spec are marked "Assumption" in the doc comment of
//! the test that makes them. Run with `cargo test --release --test
//! dynamics_atoms_acceptance`.

mod rig;

mod follower;
mod gain;
mod gain_curve;

pub use fugue::{ControlKind, ControlMeta, ControlValue, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
