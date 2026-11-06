//! Acceptance tests for the physical-model and percussive engines of the
//! `macro_oscillator` module, written from the spec alone. They drive only the
//! public API (`ModuleRegistry`, `GraphModule`, `Module`, `RenderEngine`) and
//! measure what is heard.
//!
//! The seven new models are appended to the `model` control after the seven
//! existing ones, in this order: `string`, `modal`, `bass_drum`, `snare_drum`,
//! `hi_hat`, `filtered_noise`, `particle_noise`.
//!
//! Criteria: (1) the three drums fall below -90 dBFS within 4 s of a trigger
//! at default decay, and the string and modal decay away with no growth;
//! (2) retriggering at 50% decay never steps more than 1.5x the steady-state
//! step; (3) the string is within +-5 cents from 55 to 1760 Hz, plucked and
//! un-triggered; (4) the example invention `macro_oscillator_drum_groove`
//! loads, uses only the new engines and is not silent. Plus per-model checks:
//! it builds by name, stays finite and within +-1 under random controls and
//! triggers, and `level` at 0 is below -80 dBFS. Assumptions beyond the spec
//! are marked "Assumption" in the doc comment of the test that makes them.

/// Defines one `#[test]` per name in the list, each calling `$check(name)`.
/// Used inside a module per criterion so tests read `retrigger::bass_drum`.
macro_rules! tests_for {
    ($check:path; $($name:ident),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                $check(stringify!($name));
            }
        )+
    };
}

/// A test per new engine.
macro_rules! each_new_model {
    ($check:path) => {
        tests_for!($check; string, modal, bass_drum, snare_drum, hi_hat, filtered_noise, particle_noise);
    };
}

/// A test per percussive engine (the three drums).
macro_rules! each_drum {
    ($check:path) => {
        tests_for!($check; bass_drum, snare_drum, hi_hat);
    };
}

/// A test for the string and the modal resonator, which ring on.
macro_rules! each_ringer {
    ($check:path) => {
        tests_for!($check; string, modal);
    };
}

mod rig;

mod decay;
mod example;
mod interface;
mod level_zero;
mod pitch;
mod retrigger;
mod stability;
mod trigger;

pub use fugue::{ControlKind, ControlValue, GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
