use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::*;

/// A single step in the sequencer pattern.
#[derive(Debug, Clone)]
pub struct Step {
    /// Note offset from base_note. None = rest (no note).
    pub note: Option<i8>,
    /// Gate length for this step as ratio of step duration (0.0-1.0).
    /// If None, uses the sequencer's default gate_length.
    pub gate_length: Option<f32>,
    /// Continue the previous active note without retriggering.
    pub held: bool,
    /// Optional amplitude for this step (0.0-1.0), from the score's
    /// dynamics. If None, the sequencer's velocity output stays at full
    /// (1.0).
    pub amplitude: Option<f32>,
}

impl Step {
    /// Creates a new step with a note.
    pub fn note(offset: i8) -> Self {
        Self {
            note: Some(offset),
            gate_length: None,
            held: false,
            amplitude: None,
        }
    }

    /// Creates a new step with a note and custom gate length.
    pub fn note_with_gate(offset: i8, gate_length: f32) -> Self {
        Self {
            note: Some(offset),
            gate_length: Some(gate_length.clamp(0.0, 1.0)),
            held: false,
            amplitude: None,
        }
    }

    /// Creates a rest step (no note).
    pub fn rest() -> Self {
        Self {
            note: None,
            gate_length: None,
            held: false,
            amplitude: None,
        }
    }

    /// Creates a held step that continues the previous active note.
    pub fn held() -> Self {
        Self {
            note: None,
            gate_length: None,
            held: true,
            amplitude: None,
        }
    }
}

impl Default for Step {
    fn default() -> Self {
        Self::rest()
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        parse_step(&value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Step {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.held {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("held", &true)?;
            return map.end();
        }

        let entries = 1 + usize::from(self.gate_length.is_some()) + usize::from(self.amplitude.is_some());
        let mut map = serializer.serialize_map(Some(entries))?;
        map.serialize_entry("note", &self.note)?;
        if let Some(gate_length) = self.gate_length {
            map.serialize_entry("gate", &gate_length)?;
        }
        if let Some(amplitude) = self.amplitude {
            map.serialize_entry("amplitude", &amplitude)?;
        }
        map.end()
    }
}
