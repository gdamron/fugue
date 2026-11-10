//! Acceptance tests for the `macro_oscillator` module, written from its spec
//! alone. They drive only the public API (`ModuleRegistry`, `GraphModule`,
//! `Module`) and measure what is heard.
//!
//! The module gains synthesis models a few at a time across a stack of PRs,
//! so every test enumerates the models from the `model` control's options at
//! runtime and tests each one present.
//!
//! Criteria: (1) fundamental within +-2 cents per model at 110/440/1760 Hz;
//! (2) bounded, finite, normal output under random modulation; (3) no click
//! when `model` changes; (4) level 0 is below -80 dBFS. Plus the project
//! definition of done: no-config build, declared controls and ports, and an
//! example invention. Assumptions beyond the spec are marked "Assumption" in
//! the doc comment of the test that makes them.

mod rig;

mod interface;
mod level_zero;
mod model_switch;
mod pitch;
mod stability;

pub use fugue::{ControlKind, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
