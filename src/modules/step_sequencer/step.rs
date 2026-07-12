use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::*;

/// Maximum number of grace notes a single step may carry. Longer chains are
/// trills/mordents territory, out of scope for the grace field.
pub const MAX_GRACE_NOTES: usize = 4;

/// A fixed-capacity chain of grace-note offsets decorating a note step.
///
/// Offsets are semitones from the sequencer's base note (same convention as
/// [`Step::note`]), stored in played order: the first grace sounds first and
/// the last resolves into the step's principal note. The capacity is fixed so
/// `Step` stays cheap to clone on the audio thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraceChain {
    len: u8,
    offsets: [i8; MAX_GRACE_NOTES],
}

impl GraceChain {
    /// Builds a chain from a slice of offsets in played order.
    pub fn from_slice(offsets: &[i8]) -> Result<Self, String> {
        if offsets.len() > MAX_GRACE_NOTES {
            return Err(format!(
                "at most {} grace notes per step (got {})",
                MAX_GRACE_NOTES,
                offsets.len()
            ));
        }
        let mut chain = Self::default();
        for &offset in offsets {
            chain.offsets[chain.len as usize] = offset;
            chain.len += 1;
        }
        Ok(chain)
    }

    /// Whether the chain carries no grace notes.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of grace notes in the chain.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// The offset at `index` in played order, if present.
    pub fn get(&self, index: usize) -> Option<i8> {
        (index < self.len()).then(|| self.offsets[index])
    }

    /// Iterates the offsets in played order.
    pub fn iter(&self) -> impl Iterator<Item = i8> + '_ {
        self.offsets[..self.len()].iter().copied()
    }
}

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
    /// Grace notes decorating this step, empty for most steps. Only
    /// meaningful on note steps; how the chain is realized (timing,
    /// velocity) is the sequencer's interpretation, not the pattern's.
    pub grace: GraceChain,
}

impl Step {
    /// Creates a new step with a note.
    pub fn note(offset: i8) -> Self {
        Self {
            note: Some(offset),
            gate_length: None,
            held: false,
            amplitude: None,
            grace: GraceChain::default(),
        }
    }

    /// Creates a new step with a note and custom gate length.
    pub fn note_with_gate(offset: i8, gate_length: f32) -> Self {
        Self {
            gate_length: Some(gate_length.clamp(0.0, 1.0)),
            ..Self::note(offset)
        }
    }

    /// Creates a new step with a note decorated by grace notes.
    pub fn note_with_grace(offset: i8, grace: &[i8]) -> Self {
        Self {
            grace: GraceChain::from_slice(grace).expect("too many grace notes"),
            ..Self::note(offset)
        }
    }

    /// Creates a rest step (no note).
    pub fn rest() -> Self {
        Self {
            note: None,
            gate_length: None,
            held: false,
            amplitude: None,
            grace: GraceChain::default(),
        }
    }

    /// Creates a held step that continues the previous active note.
    pub fn held() -> Self {
        Self {
            held: true,
            ..Self::rest()
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

        let entries = 1
            + usize::from(self.gate_length.is_some())
            + usize::from(self.amplitude.is_some())
            + usize::from(!self.grace.is_empty());
        let mut map = serializer.serialize_map(Some(entries))?;
        map.serialize_entry("note", &self.note)?;
        if let Some(gate_length) = self.gate_length {
            map.serialize_entry("gate", &gate_length)?;
        }
        if let Some(amplitude) = self.amplitude {
            map.serialize_entry("amplitude", &amplitude)?;
        }
        if !self.grace.is_empty() {
            let offsets: Vec<i8> = self.grace.iter().collect();
            map.serialize_entry("grace", &offsets)?;
        }
        map.end()
    }
}
