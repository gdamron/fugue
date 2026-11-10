//! Acceptance tests for the `resonator` module, written from its spec alone.
//! They drive only the public API (`ModuleRegistry`, `GraphModule`, `Module`)
//! and measure what is heard.
//!
//! The module gains models across a stack of PRs, so every test enumerates the
//! models from the `model` control's options at runtime and tests each one
//! present.
//!
//! Criteria: (1) pitch within +-3 cents per model at 110/440/1760 Hz; (2) at
//! every damping an impulse decays below -90 dBFS within the stated ring time
//! plus 1 s, with no growth; (3) no click when `model` or `polyphony` changes
//! while ringing; (4) at polyphony 4 a new strum does not cut the previous
//! tail; (5) bounded, finite, normal output under random modulation; (6) no
//! steps at host block edges during fast `structure` and `brightness` sweeps;
//! (7) interface: builds from `{}`, declared controls and ports, unknown
//! config refused, an example invention. Assumptions beyond the spec are
//! marked "Assumption" in the doc comment of the test that makes them.

mod rig;

mod decay;
mod interface;
mod pitch;
mod stability;
mod stated;
mod sweeps;
mod switching;
mod voices;

pub use fugue::{ControlKind, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
