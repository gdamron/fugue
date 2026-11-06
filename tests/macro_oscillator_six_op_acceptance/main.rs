//! Acceptance tests for the `macro_oscillator` module's `six_op_fm` model,
//! written from the spec alone. They drive only the public API
//! (`ModuleRegistry`, `GraphModule`, `Module`, `RenderEngine`) and measure
//! what is heard.
//!
//! The model position is looked up by name in the `model` control's options,
//! so until the model exists the tests fail with "model option `six_op_fm`
//! missing".
//!
//! Criteria covered (IDs appear in the doc comment of each test):
//! C1 model order; C2 in tune; C3 bounded; C4 every patch audible;
//! C5 patches differ; C6 brightness; C7 click-free macros and patch change;
//! C8 triggered notes; C9 held versus released; C10 morph stretches;
//! C11 click-free retrigger; C12 names; C13 example invention.
//! C14 (level 0 and model switches) is covered for every model by the
//! frozen `macro_oscillator_acceptance` suite and is not repeated.
//!
//! Assumptions beyond the spec are marked "Assumption" in the doc comment of
//! the test that makes them.

#![allow(clippy::needless_range_loop)]

mod rig;

mod audible;
mod bounded;
mod envelopes;
mod interface;
mod macros;
mod names;
mod patches;
mod pitch;

pub use fugue::{ControlKind, ControlValue, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
