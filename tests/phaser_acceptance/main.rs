//! Acceptance tests for the `phaser` module, written from its interface spec
//! and the issue's definition of done alone. They drive only the public API
//! (`ModuleRegistry`, `GraphModule`, `Module`) and measure what is heard:
//! steps (clicks), peak level, level against the dry input, exactness at the
//! documented identities, and the tempo-synced sweep period.
//!
//! The `phaser` type is built by name, so this suite compiles before the
//! module exists and fails at runtime until it does.
//!
//! Criteria:
//! - sweeping `rate` and `depth` over their full range in 1 s, a fast
//!   `feedback` sweep and a `stage_count` change (4, 12, 4) keep the largest
//!   sample-to-sample step of the output within 1.5x the dry input's, at
//!   block edges too (`sweep`);
//! - at feedback +/-0.95, 0 dBFS input stays within +/-1.0 (`headroom`);
//! - at default settings the RMS of the mono fold (L+R)/2 is within 3 dB of
//!   the mono input's (`level`);
//! - the interface's identities: `mix` 0 is the input exactly, `stereo_phase`
//!   0 gives identical sides, an unpatched `audio_right` copies the left, a
//!   fixed chain keeps a sine's amplitude and notches at the corner, and the
//!   sweep follows `division` and `bpm` (`behaviour`, `sync`);
//! - the declared ports and controls (`controls`).
//!
//! Assumptions beyond the spec are marked "Assumption" on the test that makes
//! them. Run with `cargo test --release --test phaser_acceptance`.

mod rig;

mod behaviour;
mod controls;
mod headroom;
mod level;
mod sweep;
mod sync;

pub use fugue::{ControlKind, ControlMeta, ControlValue, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
