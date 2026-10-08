//! Declared controls: each module type's typed table of what it accepts.
//!
//! A module that has moved onto requests declares its controls once, as a
//! [`ControlTable`]. A control thread resolves every key against it into a
//! [`ControlIndex`] and coerces every value into an [`RtValue`], so strings
//! never reach the audio thread, and a module only ever applies values of
//! the kind it declared.
//!
//! Key validity is fixed when the table is built (R5): a control with
//! `count > 1` declares `key.0` to `key.{count - 1}`, all valid for the life
//! of the module, whatever another control later makes audible.

use std::borrow::Cow;

use serde_json::Value;

use super::request::{ControlIndex, RtValue};
use crate::module_config::whole_number_in;
use crate::traits::check_finite;
use crate::{ControlKind, ControlMeta, ControlValue};

/// What a declared control holds, and so which [`RtValue`] carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DeclKind {
    /// Any finite `f32` ([`RtValue::F32`]); `min` and `max` are the range
    /// editors offer, and the module clamps as it applies.
    Number { min: f32, max: f32 },
    /// A whole number from `min` to `max` ([`RtValue::I32`]), read by the
    /// typed config reader's rules: `3.0` is 3, `3.5` is refused.
    Integer { min: i32, max: i32 },
    /// A boolean ([`RtValue::Bool`]).
    Bool,
    /// One of `options`, by name, carried as its position ([`RtValue::U32`]).
    Choice(&'static [&'static str]),
    /// A heavy value carried as a payload; no scalar write reaches it, and
    /// its module reads it back through its surface, not its cells.
    Payload,
}

/// The largest magnitude an integer control may declare: every whole number
/// up to 2^24 reads back exactly through a client's `f32`.
pub(crate) const MAX_EXACT_INTEGER: i32 = 1 << 24;

/// The most indices a table may declare: one per [`ControlIndex`].
pub(crate) const MAX_CONTROLS: usize = 1 << 16;

/// Who writes a control (R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Writer {
    /// Written by requests: control threads and automation.
    Parameter,
    /// Written only by the module's own `process()`; read-only elsewhere.
    Telemetry,
}

/// One declared control, or `count` indexed ones (`key.0`, `key.1`, ...).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlDecl {
    pub(crate) key: Cow<'static, str>,
    /// Whether it declares `key.N` for every `N < count` (any count, one
    /// or none included) rather than plain `key`.
    pub(crate) indexed: bool,
    /// How many indices it takes: 1 for a plain control.
    pub(crate) count: u16,
    pub(crate) kind: DeclKind,
    /// Whether a write fires an event (a trigger, a note) rather than sets
    /// a state: two at one sample are two events, so its requests are
    /// never coalesced, and a module adopting its state never replays one.
    pub(crate) event: bool,
    /// The unit a number is in (`"Hz"`, `"s"`), or `""`.
    pub(crate) unit: &'static str,
    /// The value a module starts from when its config leaves it out.
    pub(crate) default: RtValue,
    pub(crate) writer: Writer,
    pub(crate) description: Cow<'static, str>,
}

impl ControlDecl {
    /// A parameter written by requests, with no unit.
    pub(crate) const fn new(
        key: &'static str,
        kind: DeclKind,
        default: RtValue,
        description: &'static str,
    ) -> Self {
        Self {
            key: Cow::Borrowed(key),
            indexed: false,
            count: 1,
            kind,
            event: false,
            unit: "",
            default,
            writer: Writer::Parameter,
            description: Cow::Borrowed(description),
        }
    }

    pub(crate) const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    /// Declares `key.0` to `key.{count - 1}` instead of `key` (R5).
    pub(crate) const fn indexed(mut self, count: u16) -> Self {
        self.indexed = true;
        self.count = count;
        self
    }

    /// Makes each write an event rather than a state (see the field).
    pub(crate) const fn event(mut self) -> Self {
        self.event = true;
        self
    }

    /// Makes it telemetry: written only by the module's `process()`.
    pub(crate) const fn telemetry(mut self) -> Self {
        self.writer = Writer::Telemetry;
        self
    }
}

/// A module's declared controls, in [`ControlIndex`] order: each
/// declaration takes `count` consecutive indices.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlTable(Cow<'static, [ControlDecl]>);

impl ControlTable {
    /// A module type's fixed table.
    ///
    /// # Panics
    ///
    /// If the table is invalid (see [`invalid`]): a compile error when
    /// evaluated in a `const` or `static`.
    pub(crate) const fn of(decls: &'static [ControlDecl]) -> Self {
        if let Some(why) = invalid(decls) {
            panic!("{}", why);
        }
        Self(Cow::Borrowed(decls))
    }

    /// A table built for one instance (a development's exposed controls),
    /// or why it cannot be one.
    pub(crate) fn built(decls: Vec<ControlDecl>) -> Result<Self, &'static str> {
        match invalid(&decls) {
            Some(why) => Err(why),
            None => Ok(Self(Cow::Owned(decls))),
        }
    }

    /// How many indices the table declares.
    pub(crate) fn len(&self) -> usize {
        self.0.iter().map(|decl| usize::from(decl.count)).sum()
    }

    /// The declaration behind `index`, and the position within it.
    pub(crate) fn decl(&self, index: ControlIndex) -> Option<(&ControlDecl, u16)> {
        let mut first = 0usize;
        for decl in self.0.iter() {
            let offset = usize::from(index.0).checked_sub(first)?;
            if offset < usize::from(decl.count) {
                return Some((decl, offset as u16));
            }
            first += usize::from(decl.count);
        }
        None
    }

    /// The control `key` names (`level`, or `level.3` for an indexed one).
    pub(crate) fn resolve(&self, key: &str) -> Option<ControlIndex> {
        let mut first = 0usize;
        for decl in self.0.iter() {
            let position = if !decl.indexed {
                (key == decl.key).then_some(0)
            } else {
                key.strip_prefix(decl.key.as_ref())
                    .and_then(|rest| rest.strip_prefix('.'))
                    .filter(|n| n == &"0" || !n.starts_with('0'))
                    .and_then(|n| n.parse::<u16>().ok())
                    .filter(|n| *n < decl.count)
            };
            if let Some(position) = position {
                return u16::try_from(first + usize::from(position))
                    .ok()
                    .map(ControlIndex);
            }
            first += usize::from(decl.count);
        }
        None
    }

    /// The key of `index`, as clients write it.
    pub(crate) fn key(&self, index: ControlIndex) -> Option<String> {
        let (decl, position) = self.decl(index)?;
        Some(if !decl.indexed {
            decl.key.to_string()
        } else {
            format!("{}.{}", decl.key, position)
        })
    }

    /// Coerces a write to `index` into the value its kind carries, refusing
    /// what the kind cannot hold. Read-only controls refuse every write.
    pub(crate) fn coerce(
        &self,
        index: ControlIndex,
        value: &ControlValue,
    ) -> Result<RtValue, String> {
        let (decl, _) = self.decl(index).ok_or("Unknown control")?;
        let key = decl.key.as_ref();
        if decl.writer == Writer::Telemetry {
            return Err(format!("Control '{key}' is read-only"));
        }
        let text = match value {
            ControlValue::String(text) => Some(text.trim()),
            _ => None,
        };
        match decl.kind {
            DeclKind::Number { .. } => {
                let number = match (value, text) {
                    (ControlValue::Number(number), _) => *number,
                    (_, Some(text)) => text
                        .parse::<f32>()
                        .map_err(|_| "Expected numeric control value".to_string())?,
                    _ => value.as_number()?,
                };
                check_finite(key, number)?;
                Ok(RtValue::F32(number))
            }
            DeclKind::Integer { min, max } => {
                let number = match (value, text) {
                    (_, Some(text)) => text.parse::<f64>().ok(),
                    (ControlValue::Number(number), _) => Some(f64::from(*number)),
                    _ => None,
                };
                let json = number
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number);
                let json = json.ok_or("Expected numeric control value")?;
                whole_number_in::<i32>(&json, i128::from(min), i128::from(max))
                    .map(RtValue::I32)
                    .map_err(|refusal| format!("Control '{key}' {refusal}"))
            }
            DeclKind::Bool => {
                let flag = match (value, text) {
                    (ControlValue::Bool(flag), _) => *flag,
                    (_, Some("true")) => true,
                    (_, Some("false")) => false,
                    _ => value.as_bool()?,
                };
                Ok(RtValue::Bool(flag))
            }
            DeclKind::Choice(options) => {
                let Some(text) = text else {
                    return Err("Expected string control value".to_string());
                };
                options
                    .iter()
                    .position(|option| option.eq_ignore_ascii_case(text))
                    .map(|position| RtValue::U32(position as u32))
                    .ok_or_else(|| {
                        format!("Control '{key}' expects one of {options:?}, got '{text}'")
                    })
            }
            DeclKind::Payload => Err(format!("Control '{key}' takes a payload, not a value")),
        }
    }

    /// `value`, held by `index`, as clients read it: a choice by name.
    /// `None` for a payload, which no scalar holds.
    pub(crate) fn value(&self, index: ControlIndex, value: RtValue) -> Option<ControlValue> {
        let (decl, _) = self.decl(index)?;
        Some(match (decl.kind, value) {
            (DeclKind::Payload, _) => return None,
            (DeclKind::Choice(options), RtValue::U32(position)) => {
                ControlValue::String(options.get(position as usize)?.to_string())
            }
            (_, RtValue::F32(number)) => ControlValue::Number(number),
            (_, RtValue::I32(number)) => ControlValue::Number(number as f32),
            (_, RtValue::U32(number)) => ControlValue::Number(number as f32),
            (_, RtValue::Bool(flag)) => ControlValue::Bool(flag),
        })
    }

    /// Every declared control as clients list it, each with `current(index)`
    /// as its default (what describe shows: the value it holds now), or its
    /// declared default when `current` has none.
    pub(crate) fn metas(
        &self,
        current: impl Fn(ControlIndex) -> Option<ControlValue>,
    ) -> Vec<ControlMeta> {
        (0..self.len())
            .filter_map(|index| {
                let index = ControlIndex(index as u16);
                let (decl, _) = self.decl(index)?;
                let kind = match decl.kind {
                    DeclKind::Number { min, max } => ControlKind::Number { min, max },
                    DeclKind::Integer { min, max } => ControlKind::Number {
                        min: min as f32,
                        max: max as f32,
                    },
                    DeclKind::Bool => ControlKind::Bool,
                    DeclKind::Choice(options) => ControlKind::String {
                        options: Some(options.iter().map(|o| o.to_string()).collect()),
                    },
                    DeclKind::Payload => ControlKind::String { options: None },
                };
                let default = current(index)
                    .or_else(|| self.value(index, decl.default))
                    .unwrap_or_else(|| ControlValue::String(String::new()));
                Some(ControlMeta {
                    key: self.key(index)?,
                    description: decl.description.to_string(),
                    default,
                    kind,
                })
            })
            .collect()
    }
}

/// Why `decls` cannot be a table: more indices than a [`ControlIndex`]
/// addresses, a plain control not taking exactly one, or an integer range
/// a client's `f32` cannot read back exactly.
const fn invalid(decls: &[ControlDecl]) -> Option<&'static str> {
    let mut total = 0usize;
    let mut i = 0;
    while i < decls.len() {
        let decl = &decls[i];
        total += decl.count as usize;
        if !decl.indexed && decl.count != 1 {
            return Some("a plain control takes exactly one index");
        }
        if let DeclKind::Integer { min, max } = decl.kind {
            if min < -MAX_EXACT_INTEGER || max > MAX_EXACT_INTEGER {
                return Some("an integer control's range must lie within 2^24 of zero");
            }
        }
        i += 1;
    }
    if total > MAX_CONTROLS {
        return Some("a control table declares at most 2^16 controls");
    }
    None
}

#[cfg(test)]
mod tests;
