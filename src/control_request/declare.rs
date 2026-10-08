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
    /// An event such as a trigger ([`RtValue::Bool`]`(true)`). Two at one
    /// sample are two events: never coalesced, and never re-applied when a
    /// module adopts its declared state.
    Event,
    /// A heavy value carried as a payload; no scalar write reaches it.
    Payload,
}

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
    /// 1 for a plain control; more declares `key.N` for every `N < count`.
    pub(crate) count: u16,
    pub(crate) kind: DeclKind,
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
            count: 1,
            kind,
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
        self.count = count;
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
    pub(crate) const fn of(decls: &'static [ControlDecl]) -> Self {
        Self(Cow::Borrowed(decls))
    }

    /// A table built for one instance (a development's exposed controls).
    pub(crate) fn built(decls: Vec<ControlDecl>) -> Self {
        Self(Cow::Owned(decls))
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
            let position = if decl.count == 1 {
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
        Some(if decl.count == 1 {
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
            DeclKind::Bool | DeclKind::Event => {
                let flag = match (value, text) {
                    (ControlValue::Bool(flag), _) => *flag,
                    (_, Some("true")) => true,
                    (_, Some("false")) => false,
                    _ => value.as_bool()?,
                };
                if decl.kind == DeclKind::Event && !flag {
                    return Err(format!(
                        "Control '{key}' is an event: write true to fire it"
                    ));
                }
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
    pub(crate) fn value(&self, index: ControlIndex, value: RtValue) -> Option<ControlValue> {
        let (decl, _) = self.decl(index)?;
        Some(match (decl.kind, value) {
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
    /// as its default (what describe shows: the value it holds now).
    pub(crate) fn metas(&self, current: impl Fn(ControlIndex) -> RtValue) -> Vec<ControlMeta> {
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
                    DeclKind::Bool | DeclKind::Event => ControlKind::Bool,
                    DeclKind::Choice(options) => ControlKind::String {
                        options: Some(options.iter().map(|o| o.to_string()).collect()),
                    },
                    DeclKind::Payload => ControlKind::String { options: None },
                };
                Some(ControlMeta {
                    key: self.key(index)?,
                    description: decl.description.to_string(),
                    default: self.value(index, current(index))?,
                    kind,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
