//! The `fugue.score.v1` score data asset.
//!
//! A score is a declarative, general-purpose container for the musical content
//! of a piece — a bank of `cells`, each a sequence of steps in the same
//! `{ note, gate, held }` shape consumed by [`step_sequencer`] and
//! [`cell_sequencer`], plus light metadata (title, composer, key, tempo, time
//! signature, base-note hint, rhythm grid).
//!
//! A through-composed piece that is a single sequence is just a bank of one
//! cell (`cells: [[ ...steps... ]]`); a flat-sequence consumer can pull it via
//! the `$asset` path `/cells/0`. There is deliberately no separate flat `steps`
//! field — one canonical content shape keeps producers, validation, and the
//! import-accuracy harness simple.
//!
//! Scores are produced by score importers (e.g. an agent transcribing a PDF, or
//! a MusicXML/MIDI converter) and consumed via the invention `$asset`
//! mechanism, so the same asset file can be spliced directly into a sequencer's
//! `config`. The asset is intentionally kept general-purpose: piece-specific
//! data lives in the score file, not baked into any module.
//!
//! [`validate_score`] is the authoritative checker (mirroring the agent
//! module's `fugue.step_pattern.v1` validation in
//! `crate::agents`); the typed [`Score`] is a convenience model for producers
//! that want to build a score and serialize it.
//!
//! [`step_sequencer`]: crate::modules::StepSequencer
//! [`cell_sequencer`]: crate::modules::cell_sequencer

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::invention::format::TimeSignature;
use crate::modules::Step;

/// Current score schema identifier. A score document may carry this as its
/// `schema` field to opt into load-time validation; when absent, the document
/// is still treated as a v1 score.
pub const SCORE_SCHEMA_V1: &str = "fugue.score.v1";

/// A `fugue.score.v1` document: a piece's musical content plus light metadata.
///
/// Content is a bank of `cells`, each a sequence of steps in the shared
/// `{ note, gate, held }` shape; a single-sequence piece is a bank of one cell.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Score {
    /// Schema id; when present, must equal [`SCORE_SCHEMA_V1`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Piece title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Composer or author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,

    /// Key signature, free-form (e.g. "C minor").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// Tempo in BPM; must be positive when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f32>,

    /// Time signature (beats per measure + beat unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<TimeSignature>,

    /// Base MIDI note (0..=127) that step offsets are relative to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_note_hint: Option<i64>,

    /// Rhythmic grid the steps sit on (e.g. "16th_note"); a hint for consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rhythm_grid: Option<String>,

    /// Bank of cells, each a sequence of steps.
    pub cells: Vec<Vec<Step>>,
}

impl Score {
    /// Parses and validates a score from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
        validate_score(&value)?;
        serde_json::from_value(value).map_err(|err| err.to_string())
    }

    /// Serializes the score to a pretty JSON string.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|err| err.to_string())
    }
}

/// Validates that `value` is a well-formed `fugue.score.v1` document.
///
/// This is the authoritative checker, operating directly on JSON so it can be
/// reused by importers and by the invention asset loader before any typed model
/// is constructed. Returns a human-readable error describing the first problem
/// found.
pub fn validate_score(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "score must be a JSON object".to_string())?;

    if let Some(schema) = object.get("schema") {
        match schema.as_str() {
            Some(SCORE_SCHEMA_V1) => {}
            Some(other) => {
                return Err(format!(
                    "unsupported score schema '{}', expected '{}'",
                    other, SCORE_SCHEMA_V1
                ))
            }
            None => return Err("score.schema must be a string".to_string()),
        }
    }

    for key in ["title", "composer", "key", "rhythm_grid"] {
        if let Some(field) = object.get(key) {
            if !field.is_null() && !field.is_string() {
                return Err(format!("score.{} must be a string", key));
            }
        }
    }

    if let Some(tempo) = object.get("tempo").filter(|v| !v.is_null()) {
        let tempo = tempo
            .as_f64()
            .ok_or_else(|| "score.tempo must be a number".to_string())?;
        if !(tempo.is_finite() && tempo > 0.0) {
            return Err("score.tempo must be a positive number".to_string());
        }
    }

    if let Some(base) = object.get("base_note_hint").filter(|v| !v.is_null()) {
        let base = base
            .as_i64()
            .ok_or_else(|| "score.base_note_hint must be an integer".to_string())?;
        if !(0..=127).contains(&base) {
            return Err("score.base_note_hint must be between 0 and 127".to_string());
        }
    }

    if let Some(time_signature) = object.get("time_signature").filter(|v| !v.is_null()) {
        validate_time_signature(time_signature)?;
    }

    let cells = object
        .get("cells")
        .filter(|v| !v.is_null())
        .ok_or_else(|| "score must contain a non-empty 'cells' array".to_string())?
        .as_array()
        .ok_or_else(|| "score.cells must be an array".to_string())?;
    if cells.is_empty() {
        return Err("score.cells must not be empty".to_string());
    }
    for (index, cell) in cells.iter().enumerate() {
        let steps = cell
            .as_array()
            .ok_or_else(|| format!("score.cells[{}] must be an array of steps", index))?;
        if steps.is_empty() {
            return Err(format!("score.cells[{}] must not be empty", index));
        }
        for step in steps {
            validate_step(step).map_err(|err| format!("score.cells[{}]: {}", index, err))?;
        }
    }

    Ok(())
}

fn validate_time_signature(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "score.time_signature must be an object".to_string())?;
    for key in ["beats_per_measure", "beat_unit"] {
        let field = object
            .get(key)
            .ok_or_else(|| format!("score.time_signature.{} is required", key))?;
        let number = field
            .as_u64()
            .ok_or_else(|| format!("score.time_signature.{} must be a positive integer", key))?;
        if number == 0 {
            return Err(format!(
                "score.time_signature.{} must be greater than 0",
                key
            ));
        }
    }
    Ok(())
}

/// Validates a single step against the `{ note, gate, held }` shape shared with
/// the sequencers. Accepts the three forms their parser accepts: `null` (rest),
/// a bare integer (note offset), or an object.
fn validate_step(value: &Value) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }

    if let Some(note) = value.as_i64() {
        return check_note_range(note);
    }

    let object = value
        .as_object()
        .ok_or_else(|| "each step must be null, an integer, or an object".to_string())?;

    // A held step continues the previous note and may carry nothing else.
    match object.get("held") {
        Some(Value::Bool(true)) => {
            if object.keys().any(|key| key != "held") {
                return Err("held steps may only contain {\"held\": true}".to_string());
            }
            return Ok(());
        }
        Some(Value::Bool(false)) | None => {}
        Some(_) => return Err("step.held must be a boolean".to_string()),
    }

    match object.get("note") {
        Some(Value::Null) | None => {}
        Some(Value::Number(number)) => {
            let note = number
                .as_i64()
                .ok_or_else(|| "step.note must be an integer or null".to_string())?;
            check_note_range(note)?;
        }
        Some(_) => return Err("step.note must be an integer or null".to_string()),
    }

    if let Some(gate) = object.get("gate").filter(|v| !v.is_null()) {
        let gate = gate
            .as_f64()
            .ok_or_else(|| "step.gate must be a number".to_string())?;
        if !(0.0..=1.0).contains(&gate) {
            return Err("step.gate must be between 0 and 1".to_string());
        }
    }

    Ok(())
}

/// Step note offsets are stored as `i8` by the sequencers, so they must fit.
fn check_note_range(note: i64) -> Result<(), String> {
    if (i8::MIN as i64..=i8::MAX as i64).contains(&note) {
        Ok(())
    } else {
        Err(format!(
            "step.note {} out of range (must fit in -128..=127)",
            note
        ))
    }
}

pub mod compare;
#[cfg(feature = "score-import")]
pub mod import;

pub use compare::{compare_scores, CompareReport};

#[cfg(test)]
mod tests;
