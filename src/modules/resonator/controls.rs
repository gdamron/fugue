//! The resonator's declared controls.

use super::voices::{MODELS, VOICES};
use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const MODEL: ControlIndex = ControlIndex(0);
pub(super) const FREQUENCY: ControlIndex = ControlIndex(1);
pub(super) const STRUCTURE: ControlIndex = ControlIndex(2);
pub(super) const BRIGHTNESS: ControlIndex = ControlIndex(3);
pub(super) const DAMPING: ControlIndex = ControlIndex(4);
pub(super) const POSITION: ControlIndex = ControlIndex(5);
pub(super) const POLYPHONY: ControlIndex = ControlIndex(6);
pub(super) const LEVEL: ControlIndex = ControlIndex(7);

/// A 0-to-1 timbre control.
const fn shape(key: &'static str, default: f32, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(default),
        description,
    )
    .precision(3)
    .clamped(0.0, 1.0)
}

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "model",
        DeclKind::Choice(MODELS),
        RtValue::U32(0),
        "What rings: modal (a struck body: bowl, bar, bell), sympathetic_strings (a string with a halo of strings tuned around it), string (a plucked string that structure makes stiff or buzzing). A change lets the old body ring out",
    ),
    ControlDecl::new(
        "frequency",
        DeclKind::Number {
            min: 20.0,
            max: 20000.0,
        },
        RtValue::F32(220.0),
        "Pitch in Hz of the current voice; earlier voices keep theirs",
    )
    .unit("Hz")
    .log()
    .precision(1)
    .clamped(0.0, f32::MAX),
    shape(
        "structure",
        0.25,
        "modal: partials from crowded (0) through harmonic (0.25-0.3) to bell and metal (1); sympathetic_strings: the intervals the halo is tuned to; string: buzzing bridge below 0.24, plain at 0.25, stiff piano-like above 0.26",
    ),
    shape(
        "brightness",
        0.5,
        "How bright the strike and the ring are; low values dull the upper partials quickly",
    ),
    shape(
        "damping",
        0.5,
        "How long it rings: the fundamental falls 60 dB in 0.07 s at 0, about 4.5 s at 0.5 and 18 s at 1",
    ),
    shape(
        "position",
        0.3,
        "Where it is struck or picked, from the end (0) to the middle (0.5) and back; moves the hollow, comb-like colour",
    ),
    ControlDecl::new(
        "polyphony",
        DeclKind::Integer {
            min: 1,
            max: VOICES as i32,
        },
        RtValue::I32(1),
        "Voices that ring at once (1 to 4); each strum moves to the next voice and earlier ones ring on. At 1 the odd and even outputs carry odd and even partials; above 1 voices alternate between them (at 3, the third keeps its partials split across both)",
    )
    .clamped(1.0, VOICES as f32),
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Output level, 0 to 1",
    )
    .clamped(0.0, 1.0),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
