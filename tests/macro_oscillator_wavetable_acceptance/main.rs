//! Acceptance tests for the `macro_oscillator` module's `wavetable` model and
//! the wavetable half of its `chords` model, written from the spec alone.
//! They drive only the public API (`ModuleRegistry`, `GraphModule`, `Module`,
//! `RenderEngine`) and measure what is heard.
//!
//! The module gains models a few at a time across parallel work, so the
//! `wavetable` position is looked up by name in the `model` control's options.
//! Until the model exists the tests fail with "model option `wavetable`
//! missing".
//!
//! Criteria covered (IDs appear in the doc comment of each test):
//! - A1 model order; A2 in tune; A3 bounded; A4 click-free morphing;
//!   A5 band-limited; A6 `aux` is 5-bit; A7 example invention.
//! - A8 (level 0 silences every model) is already covered for every model by
//!   the frozen `macro_oscillator_acceptance` suite and is not repeated.
//! - B1 chords wavetable half in tune and audible; B2 chords morph sweep is
//!   click-free; B3 the chords halves sound different.
//! - Bank layout (spec preamble): `timbre` = 1 is the brightest column of every
//!   bank, the four banks differ, the lower half of `harmonics` glides and the
//!   upper half steps.
//!
//! Assumptions beyond the spec are marked "Assumption" in the doc comment of
//! the test that makes them.

mod rig;

mod aux_bits;
mod banks;
mod bounded;
mod chords;
mod interface;
mod morph;
mod pitch;
mod spectrum_checks;

pub use fugue::{ControlKind, ControlValue, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
