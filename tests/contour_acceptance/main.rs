//! Acceptance tests for the `contour` module, written from its spec alone.
//! They drive only the public API (`ModuleRegistry`, `GraphModule`,
//! `Module`) at 48 kHz and measure what comes out.
//!
//! Criteria: (1) AD and AR stage times within 1% or 1 sample; (2) clock-locked
//! ratio outputs stay phase-locked through a 90 -> 140 bpm tempo change;
//! (3) outputs stay within +-1 (and unipolar where specified) under random
//! modulation of every control and input; (4) a mode or range change steps no
//! more than 1.5x the steady-state step; (5) aliasing at about 1 kHz at least
//! 20 dB below a naive ramp.
//!
//! Cycle starts of a looping output are found as the local minimum at the end
//! of a steep fall (`slope` 0.98), where the ramp restarts.

mod rig;

mod aliasing;
mod bounds;
mod clock_lock;
mod mode_switch;
mod stage_times;

pub use fugue::{ControlKind, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
