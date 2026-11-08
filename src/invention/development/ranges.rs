//! The ranges a development's exposed controls declare.
//!
//! An exposed number control takes its first alias's range unless one of
//! its entries declares a narrower one (a chorus's `rate` reaching an lfo
//! that runs to 100 Hz, where a chorus garbles). The control then reports
//! that range, and holds its starting value and every write within it, as
//! a module's own control holds writes within the range it clamps to. A
//! range must lie within each inner control the name reaches, so no alias
//! is asked to hold a value its own range refuses.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use crate::control_request::{ControlDecl, DeclKind};
use crate::{ControlKind, ControlValue, Invention};

/// What the entries of one exposed control declare.
enum Declared {
    /// One range, which every entry declaring one agrees on.
    Range((f32, f32)),
    /// The first range declared and the first one differing from it.
    Differing((f32, f32), (f32, f32)),
}

/// The ranges a development's exposed controls declare, found in one pass
/// over its definition so each lookup is a map read, however many controls
/// it exposes.
pub(super) struct Ranges<'a> {
    development: &'a str,
    declared: HashMap<&'a str, Declared>,
}

impl<'a> Ranges<'a> {
    /// The ranges `definition`, the development `development`, declares,
    /// by exposed control name, from whichever of the name's entries
    /// declare one.
    pub(super) fn of(development: &'a str, definition: &'a Invention) -> Self {
        let mut declared = HashMap::new();
        for control in &definition.controls {
            let Some(range) = control.range else {
                continue;
            };
            let range = (range.min as f32, range.max as f32);
            match declared.entry(control.name.as_str()) {
                Entry::Vacant(vacant) => {
                    vacant.insert(Declared::Range(range));
                }
                Entry::Occupied(mut occupied) => {
                    if let Declared::Range(first) = *occupied.get() {
                        if first != range {
                            occupied.insert(Declared::Differing(first, range));
                        }
                    }
                }
            }
        }
        Self {
            development,
            declared,
        }
    }

    /// The range exposed control `name` declares; or why it cannot hold
    /// one: entries declaring different ranges.
    pub(super) fn get(&self, name: &str) -> Result<Option<(f32, f32)>, String> {
        match self.declared.get(name) {
            None => Ok(None),
            Some(Declared::Range(range)) => Ok(Some(*range)),
            Some(Declared::Differing(first, other)) => Err(format!(
                "Development '{}' control '{name}': its entries declare different \
                 ranges, {} to {} and {} to {}",
                self.development, first.0, first.1, other.0, other.1
            )),
        }
    }
}

/// Refuses `range`, declared for exposed control `name` of `development`,
/// unless it narrows `alias`, the inner control `decl` declares: a number
/// control whose own range holds it, from a finite min up to a finite max,
/// whole numbers for a control held whole (so what it reads back is what
/// it plays).
pub(super) fn check(
    development: &str,
    name: &str,
    (min, max): (f32, f32),
    alias: &str,
    decl: &ControlDecl,
) -> Result<(), String> {
    let prefix = format!("Development '{development}' control '{name}'");
    let DeclKind::Number {
        min: own_min,
        max: own_max,
    } = decl.kind
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
    if decl.whole && (min.fract() != 0.0 || max.fract() != 0.0) {
        return Err(format!(
            "{prefix}: range {min} to {max} must run between whole numbers, since \
             '{alias}' holds whole numbers (its range is {own_min} to {own_max})"
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
