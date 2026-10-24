//! The melody's controls, all declared: its scalars, one degree and one
//! weight per scale position, and the whole scale and whole weights as
//! payloads built on a control thread. Every one changes only as the audio
//! thread applies a request, so a write never waits and never races
//! another: they land one at a time, in order.

use serde_json::Value;

use crate::control_request::{
    ControlContext, ControlDecl, ControlIndex, ControlTable, DeclKind, PayloadCodec, Prepared,
    RtValue,
};
use crate::module_config::{finite_f32, whole_number};
use crate::payload::Payload;
use crate::ControlValue;

/// Maximum number of scale positions a melody can choose between.
pub(crate) const MAX_DEGREES: usize = 128;

pub(super) const ROOT_NOTE: ControlIndex = ControlIndex(0);
pub(super) const DEGREE_COUNT: ControlIndex = ControlIndex(1);
pub(super) const SEED: ControlIndex = ControlIndex(2);
pub(super) const DEGREES: ControlIndex = ControlIndex(3);
pub(super) const NOTE_WEIGHTS: ControlIndex = ControlIndex(4);
const FIRST_DEGREE: u16 = 5;
const FIRST_WEIGHT: u16 = FIRST_DEGREE + MAX_DEGREES as u16;

/// The seed as a number: f32 controls hold integers exactly to 2^24.
const SEED_LIMIT: f32 = 16_777_216.0;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "root_note",
        DeclKind::Number {
            min: 0.0,
            max: 127.0,
        },
        RtValue::F32(60.0),
        "Root MIDI note number",
    )
    .clamped(0.0, 127.0)
    .whole(),
    ControlDecl::new(
        "degree_count",
        DeclKind::Number {
            min: 1.0,
            max: MAX_DEGREES as f32,
        },
        RtValue::F32(7.0),
        "Number of active scale degrees",
    )
    .clamped(1.0, MAX_DEGREES as f32)
    .whole(),
    ControlDecl::new(
        "seed",
        DeclKind::Number {
            min: 0.0,
            max: SEED_LIMIT,
        },
        RtValue::F32(0.0),
        "RNG seed: same seed, same melody; setting it (re)starts the stream. \
         Note: f32 controls carry ~24 bits of integer precision; use module \
         config JSON for full 64-bit seeds.",
    )
    // Only from below: the melody seeds from any number written.
    .clamped(0.0, f32::MAX)
    .whole(),
    ControlDecl::payload(
        "degrees",
        PayloadCodec {
            prepare: prepare_degrees,
        },
        "The scale as a JSON array of degrees: sets the count to its length \
         and forgets degrees written to single positions",
    ),
    ControlDecl::payload(
        "note_weights",
        PayloadCodec {
            prepare: prepare_weights,
        },
        "Base weights as a JSON array: forgets weights written to single positions",
    ),
    ControlDecl::new(
        "degree",
        DeclKind::Number {
            min: -127.0,
            max: 127.0,
        },
        RtValue::F32(0.0),
        "Scale degree at a position",
    )
    .clamped(-127.0, 127.0)
    .whole()
    .indexed(MAX_DEGREES as u16)
    .listed_below("degree_count"),
    ControlDecl::new(
        "note_weight",
        DeclKind::Number {
            min: 0.0,
            max: 10.0,
        },
        RtValue::F32(1.0),
        "Probability weight of a position",
    )
    .clamped(0.0, 10.0)
    .indexed(MAX_DEGREES as u16)
    .listed_below("degree_count"),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS
        .iter()
        .flat_map(|decl| std::iter::repeat_n(decl.default, usize::from(decl.count)))
}

pub(super) fn degree_index(position: usize) -> ControlIndex {
    ControlIndex(FIRST_DEGREE + position as u16)
}

pub(super) fn weight_index(position: usize) -> ControlIndex {
    ControlIndex(FIRST_WEIGHT + position as u16)
}

/// Which position a single-position control is: `(false, i)` for
/// `degree.i`, `(true, i)` for `note_weight.i`.
pub(super) fn position(control: ControlIndex) -> Option<(bool, usize)> {
    let index = usize::from(control.0);
    let first = usize::from(FIRST_DEGREE);
    match index.checked_sub(first)? {
        offset if offset < MAX_DEGREES => Some((false, offset)),
        offset if offset < 2 * MAX_DEGREES => Some((true, offset - MAX_DEGREES)),
        _ => None,
    }
}

/// A whole scale, as written: its first [`MAX_DEGREES`] degrees.
pub(super) struct Scale(pub(super) Vec<i32>);

/// Whole base weights, as written: the first [`MAX_DEGREES`].
pub(super) struct Weights(pub(super) Vec<f32>);

/// Reads `values` (an array, or its JSON text: the form a recorded write
/// leaves) as a list of `read`, refusing an element by its path
/// (`degrees[2]`), and keeps the first [`MAX_DEGREES`].
pub(super) fn list<T>(
    key: &str,
    values: &Value,
    read: impl Fn(&Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let parsed;
    let values = match values {
        Value::String(text) => {
            parsed = serde_json::from_str::<Value>(text).map_err(|error| error.to_string())?;
            &parsed
        }
        values => values,
    };
    let Some(values) = values.as_array() else {
        return Err(format!("'{key}' expects an array of numbers, got {values}"));
    };
    let mut list = Vec::with_capacity(values.len().min(MAX_DEGREES));
    for (index, value) in values.iter().take(MAX_DEGREES).enumerate() {
        list.push(read(value).map_err(|refusal| format!("'{key}[{index}]' {refusal}"))?);
    }
    Ok(list)
}

pub(super) fn degree_of(value: &Value) -> Result<i32, String> {
    whole_number::<i32>(value).map_err(|refusal| refusal.to_string())
}

pub(super) fn weight_of(value: &Value) -> Result<f32, String> {
    finite_f32(value).map_err(|refusal| refusal.to_string())
}

/// A list as its control reads back: JSON text.
pub(super) fn shown<T: serde::Serialize>(list: &[T]) -> ControlValue {
    serde_json::to_string(list)
        .unwrap_or_else(|_| "[]".to_string())
        .into()
}

fn prepare_degrees(_: &ControlContext<'_>, value: &ControlValue) -> Prepared {
    let degrees = list(
        "degrees",
        &Value::String(value.as_string()?.into()),
        degree_of,
    )?;
    let shown = shown(&degrees);
    Ok((Payload::new(Scale(degrees)), shown))
}

fn prepare_weights(_: &ControlContext<'_>, value: &ControlValue) -> Prepared {
    let weights = list(
        "note_weights",
        &Value::String(value.as_string()?.into()),
        weight_of,
    )?;
    let shown = shown(&weights);
    Ok((Payload::new(Weights(weights)), shown))
}
