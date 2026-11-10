//! Builders for [`ControlMeta`].

use super::{ControlKind, ControlMeta, ControlScale, ControlValue, NumberHints};

impl ControlMeta {
    /// Legacy alias for creating a numeric control metadata entry.
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self::number(key, description)
    }

    /// Creates a numeric control metadata entry.
    pub fn number(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::Number(0.0),
            kind: ControlKind::Number {
                min: 0.0,
                max: 1.0,
                hints: NumberHints::default(),
            },
            read_only: false,
            event: false,
        }
    }

    /// Sets the range (min, max) for this control, keeping its hints.
    pub fn with_range(mut self, min: f32, max: f32) -> Self {
        let hints = match self.kind {
            ControlKind::Number { hints, .. } => hints,
            _ => NumberHints::default(),
        };
        self.kind = ControlKind::Number { min, max, hints };
        self
    }

    /// Sets the unit a number is shown in (`"Hz"`, `"s"`).
    pub fn with_unit(self, unit: impl Into<String>) -> Self {
        let unit = unit.into();
        self.with_hints(|hints| hints.unit = Some(unit))
    }

    /// Marks a number as shown on a log scale (frequencies, times). Its
    /// range must start above zero.
    pub fn with_log_scale(self) -> Self {
        self.with_hints(|hints| hints.scale = Some(ControlScale::Log))
    }

    /// Sets a number's keyboard step, in its unit.
    pub fn with_step(self, step: f32) -> Self {
        self.with_hints(|hints| hints.step = Some(step))
    }

    /// Sets how many decimal places a number shows.
    pub fn with_precision(self, precision: u8) -> Self {
        self.with_hints(|hints| hints.precision = Some(precision))
    }

    /// Edits a number's hints; other kinds have none, and are unchanged.
    fn with_hints(mut self, edit: impl FnOnce(&mut NumberHints)) -> Self {
        if let ControlKind::Number { hints, .. } = &mut self.kind {
            edit(hints);
        }
        self
    }

    /// Marks a control as written only by its module (a status, a playhead):
    /// it is listed read-only, and every write to it is refused.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Sets the default value for this control.
    pub fn with_default(mut self, default: impl Into<ControlValue>) -> Self {
        self.default = default.into();
        self
    }

    /// Creates a boolean control metadata entry.
    pub fn boolean(key: impl Into<String>, description: impl Into<String>, default: bool) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::Bool(default),
            kind: ControlKind::Bool,
            read_only: false,
            event: false,
        }
    }

    /// Creates a string control metadata entry.
    pub fn string(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::String(String::new()),
            kind: ControlKind::String { options: None },
            read_only: false,
            event: false,
        }
    }

    /// Sets the allowed values for a string control.
    pub fn with_options(mut self, options: Vec<String>) -> Self {
        let default_option = options.first().cloned().unwrap_or_else(String::new);
        self.kind = ControlKind::String {
            options: Some(options),
        };
        if !matches!(self.default, ControlValue::String(_)) {
            self.default = ControlValue::String(default_option);
        }
        self
    }

    /// Legacy alias for enumerated controls.
    pub fn with_variants(self, variants: Vec<String>) -> Self {
        self.with_options(variants)
    }
}
