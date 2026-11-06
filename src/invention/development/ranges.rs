//! The ranges a development's exposed controls declare.
//!
//! An exposed number control takes its first alias's range unless one of
//! its entries declares a narrower one (a chorus's `rate` reaching an lfo
//! that runs to 100 Hz, where a chorus garbles). The control then reports
//! that range, and holds its starting value and every write within it, as
//! a module's own control holds writes within the range it clamps to. A
//! range must lie within each inner control the name reaches, so no alias
//! is asked to hold a value its own range refuses.

use crate::control_request::{DeclKind, RtValue};
use crate::{ControlKind, ControlValue, Invention};

/// The range `definition`, the development `development`, declares for its
/// exposed control `name`, from whichever of the name's entries declare
/// one; or why it cannot hold one: entries declaring different ranges.
pub(super) fn declared(
    development: &str,
    definition: &Invention,
    name: &str,
) -> Result<Option<(f32, f32)>, String> {
    let mut ranges = (definition.controls.iter())
        .filter(|control| control.name == name)
        .filter_map(|control| control.range)
        .map(|range| (range.min as f32, range.max as f32));
    let Some(first) = ranges.next() else {
        return Ok(None);
    };
    match ranges.find(|other| *other != first) {
        Some(other) => Err(format!(
            "Development '{development}' control '{name}': its entries declare different \
             ranges, {} to {} and {} to {}",
            first.0, first.1, other.0, other.1
        )),
        None => Ok(Some(first)),
    }
}

/// Refuses `range`, declared for exposed control `name` of `development`,
/// unless it narrows `alias`, an inner control of kind `kind`: a number
/// control whose own range holds it, from a finite min up to a finite max.
pub(super) fn check(
    development: &str,
    name: &str,
    (min, max): (f32, f32),
    alias: &str,
    kind: DeclKind,
) -> Result<(), String> {
    let prefix = format!("Development '{development}' control '{name}'");
    let DeclKind::Number {
        min: own_min,
        max: own_max,
    } = kind
    else {
        return Err(format!(
            "{prefix}: a range ({min} to {max}) narrows only a number control, and '{alias}' \
             is not one"
        ));
    };
    if !(min.is_finite() && max.is_finite()) || min > max {
        return Err(format!(
            "{prefix}: range {min} to {max} needs a finite min no greater than its max \
             (the range of '{alias}' is {own_min} to {own_max})"
        ));
    }
    if min < own_min || max > own_max {
        return Err(format!(
            "{prefix}: range {min} to {max} lies outside the range of '{alias}', \
             {own_min} to {own_max}"
        ));
    }
    Ok(())
}

/// Refuses `range`, declared for exposed control `name` of `development`,
/// when the value its alias `alias` starts at (`current`, from the
/// definition or the instance's config) lies outside it: a definition
/// starting a control where its own range forbids is written wrongly.
pub(super) fn check_start(
    development: &str,
    name: &str,
    (min, max): (f32, f32),
    alias: &str,
    current: RtValue,
) -> Result<(), String> {
    match current {
        RtValue::F32(value) if !(min..=max).contains(&value) => Err(format!(
            "Development '{development}' control '{name}': '{alias}' starts at {value}, \
             outside its range {min} to {max}"
        )),
        _ => Ok(()),
    }
}

/// Why `range`, declared for exposed control `name` of `development`, is
/// refused when one of its aliases, `alias`, is not declared: its writes
/// reach that alias on a control thread, never through the development's
/// own clamp.
pub(super) fn undeclared(
    development: &str,
    name: &str,
    (min, max): (f32, f32),
    alias: &str,
) -> String {
    format!(
        "Development '{development}' control '{name}': a range ({min} to {max}) narrows \
         only a declared number control, and '{alias}' is not one"
    )
}

/// `kind`, the kind an exposed control lists, narrowed to `range`.
pub(super) fn listed(kind: ControlKind, range: Option<(f32, f32)>) -> ControlKind {
    match (kind, range) {
        (ControlKind::Number { hints, .. }, Some((min, max))) => {
            ControlKind::Number { min, max, hints }
        }
        (kind, _) => kind,
    }
}

/// `value`, an instance's starting value for a control declaring `range`,
/// held within it.
pub(super) fn held(value: ControlValue, range: Option<(f32, f32)>) -> ControlValue {
    match (value, range) {
        (ControlValue::Number(number), Some((min, max))) => {
            ControlValue::Number(number.max(min).min(max))
        }
        (value, _) => value,
    }
}
