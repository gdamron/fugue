//! Acceptance tests for the `reverb` module, written from its behavior spec
//! alone. They drive only the public API (`Module`, `ModuleFactory`,
//! `RenderEngine`) and measure what is heard: decay time, stability, click
//! freedom, stereo decorrelation and the control surface.
//!
//! Spec summary: RT60 = 0.2 + 14.8 * decay seconds; room_size is a nominal
//! 1 m..100 m room; damping removes highs (0 = none); wet/width law is
//! same-side wet * (0.5 + 0.5 * width), cross wet * 0.5 * (1 - width);
//! freeze mutes the input, makes feedback lossless and bypasses damping;
//! room_size, decay and freeze changes crossfade without clicks.
//!
//! Assumption: for equal controls and input the reverb is deterministic, so
//! two fresh instances render identical audio.
//!
//! These renders are long; run with `cargo test --release --test
//! reverb_acceptance`.

mod rig;

mod clicks;
mod controls;
mod decay;
mod stability;
mod stereo;

pub use fugue::modules::{Reverb, ReverbFactory};
pub use fugue::{ControlKind, ControlValue, GraphModule, Module, ModuleFactory, RenderEngine};
pub use serde_json::{json, Map, Value};
