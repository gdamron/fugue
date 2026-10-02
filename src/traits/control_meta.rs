//! Builders for [`ControlMeta`].

use super::{ControlKind, ControlMeta, ControlValue};

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
            kind: ControlKind::Number { min: 0.0, max: 1.0 },
        }
    }

    /// Sets the range (min, max) for this control.
    pub fn with_range(mut self, min: f32, max: f32) -> Self {
        self.kind = ControlKind::Number { min, max };
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
        }
    }

    /// Creates a string control metadata entry.
    pub fn string(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            default: ControlValue::String(String::new()),
            kind: ControlKind::String { options: None },
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
