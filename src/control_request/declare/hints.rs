//! A declared number's presentation hints: how editors show and step it.
//!
//! Hints are metadata only (see [`crate::NumberHints`]): they never change
//! what a control accepts or how its module applies it. A whole-number
//! control steps by 1 and shows no decimals unless it declares otherwise.

use super::{ControlDecl, DeclKind};
use crate::{ControlScale, NumberHints};

impl ControlDecl {
    /// The unit a number is in (`"Hz"`, `"s"`, `"BPM"`).
    pub(crate) const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    /// Shows a number on a log scale: equal travel for equal ratios, as
    /// for frequencies and times. Its range must start above zero.
    pub(crate) const fn log(mut self) -> Self {
        self.scale = Some(ControlScale::Log);
        self
    }

    /// A number's keyboard step, in its unit.
    pub(crate) const fn step(mut self, step: f32) -> Self {
        self.step = Some(step);
        self
    }

    /// How many decimal places a number shows.
    pub(crate) const fn precision(mut self, precision: u8) -> Self {
        self.precision = Some(precision);
        self
    }

    /// The hints clients list for it.
    pub(super) fn hints(&self) -> NumberHints {
        let whole = matches!(self.kind, DeclKind::Integer { .. });
        NumberHints {
            unit: (!self.unit.is_empty()).then(|| self.unit.to_string()),
            scale: self.scale,
            step: self.step.or(whole.then_some(1.0)),
            precision: self.precision.or(whole.then_some(0)),
        }
    }
}

/// Why `decl`'s hints cannot stand: hints on a control that is not a
/// number, a log scale over a range reaching zero, or a step that is not a
/// positive finite number.
pub(super) const fn invalid(decl: &ControlDecl) -> Option<&'static str> {
    let hinted = !decl.unit.is_empty()
        || decl.scale.is_some()
        || decl.step.is_some()
        || decl.precision.is_some();
    let lowest = match decl.kind {
        DeclKind::Number { min, .. } => min,
        DeclKind::Integer { min, .. } => min as f32,
        _ if hinted => return Some("only a number control takes presentation hints"),
        _ => return None,
    };
    if matches!(decl.scale, Some(ControlScale::Log)) && (lowest.is_nan() || lowest <= 0.0) {
        return Some("a log scale needs a range starting above zero");
    }
    if let Some(step) = decl.step {
        if !step.is_finite() || step <= 0.0 {
            return Some("a step must be a positive finite number");
        }
    }
    None
}

#[cfg(test)]
mod tests;
