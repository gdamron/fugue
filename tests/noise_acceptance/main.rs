//! Acceptance tests for the `noise` module, written from its behavior spec
//! alone. They drive only the public API (`ModuleRegistry`, `GraphModule`,
//! `Module`) and measure what is heard.
//!
//! Spec summary: config `seed` (unsigned integer, optional); outputs `white`
//! and `pink`; control `level` from 0 to 1 (default 1). At level 1 white has
//! a mean within 0.01 of zero and an RMS of 0.577 +/- 0.02 over 10 s at
//! 48 kHz. Pink slopes -3 dB/oct +/- 1 dB from 50 Hz to 10 kHz. The same seed
//! gives bit-identical output. Level 0 silences both outputs, and the outputs
//! stay finite and bounded over long renders. An example invention using the
//! module ships under `examples/` and is listed in the example catalog.
//!
//! Assumptions beyond the spec are marked "Assumption" in the doc comment of
//! the test that makes them.

mod rig;

mod catalog;
mod level;
mod measurement_check;
mod pink;
mod seed;
mod stability;
mod white;

pub use fugue::{ControlKind, ControlValue, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
