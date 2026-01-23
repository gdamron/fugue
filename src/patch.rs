//! Declarative patch file format for defining synthesis setups.
//!
//! Patches are JSON documents that describe modules and their connections.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A complete patch document defining a modular synthesis setup.
///
/// Patches can be loaded from JSON files or strings and define
/// the modules to instantiate and how they connect together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    /// Version of the patch format.
    #[serde(default = "default_version")]
    pub version: String,

    /// Optional title for the patch.
    #[serde(default)]
    pub title: Option<String>,

    /// Optional description of the patch.
    #[serde(default)]
    pub description: Option<String>,

    /// The modules in this patch.
    pub modules: Vec<ModuleSpec>,

    /// Connections between modules (patch cables).
    pub connections: Vec<Connection>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Patch {
    /// Parses a patch from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(json)?)
    }

    /// Loads a patch from a JSON file.
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
    }

    /// Serializes the patch to a JSON string.
    pub fn to_json(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// Specification for a single module in a patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSpec {
    /// Unique identifier for this module instance.
    pub id: String,

    /// Type of module (e.g., "clock", "melody", "voice", "dac").
    #[serde(rename = "type")]
    pub module_type: String,

    /// Module-specific configuration.
    #[serde(default)]
    pub config: ModuleConfig,
}

/// Configuration parameters for a module.
///
/// Contains optional fields for various module types. Unused fields
/// are simply ignored for a given module type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModuleConfig {
    /// Tempo in beats per minute (for clock modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,

    /// Time signature configuration (for clock modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<TimeSignature>,

    /// Root note as MIDI number (for melody modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_note: Option<u8>,

    /// Scale mode name (for melody modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Allowed scale degrees (for melody modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_degrees: Option<Vec<usize>>,

    /// Probability weights for each scale degree (for melody modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_weights: Option<Vec<f32>>,

    /// Note duration in beats (for melody modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_duration: Option<f32>,

    /// Oscillator waveform type (for voice/oscillator modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oscillator_type: Option<String>,

    /// Base frequency in Hz (for oscillator modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<f32>,

    /// Frequency modulation depth in Hz (for oscillator modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fm_amount: Option<f32>,

    /// Amplitude modulation depth 0.0-1.0 (for oscillator modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub am_amount: Option<f32>,

    /// Filter cutoff frequency in Hz (for filter modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<f32>,

    /// Filter resonance 0.0-1.0 (for filter modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resonance: Option<f32>,

    /// ADSR attack time in seconds (for adsr modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack: Option<f32>,

    /// ADSR decay time in seconds (for adsr modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay: Option<f32>,

    /// ADSR sustain level 0.0-1.0 (for adsr modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sustain: Option<f32>,

    /// ADSR release time in seconds (for adsr modules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<f32>,

    /// Additional fields for extensibility.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Time signature specification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeSignature {
    /// Number of beats per measure (numerator).
    pub beats_per_measure: u32,
    /// Note value that gets one beat (denominator).
    pub beat_unit: u32,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            beats_per_measure: 4,
            beat_unit: 4,
        }
    }
}

/// A connection between two modules in a patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// Source module ID.
    pub from: String,

    /// Destination module ID.
    pub to: String,

    /// Optional output port name for modules with multiple outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_port: Option<String>,

    /// Optional input port name for modules with multiple inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_port: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_patch() {
        let json = r#"
        {
            "version": "1.0.0",
            "title": "Test Patch",
            "modules": [
                {
                    "id": "clock1",
                    "type": "clock",
                    "config": {
                        "bpm": 120.0,
                        "time_signature": {
                            "beats_per_measure": 4,
                            "beat_unit": 4
                        }
                    }
                }
            ],
            "connections": []
        }
        "#;

        let patch = Patch::from_json(json).unwrap();
        assert_eq!(patch.version, "1.0.0");
        assert_eq!(patch.title, Some("Test Patch".to_string()));
        assert_eq!(patch.modules.len(), 1);
        assert_eq!(patch.modules[0].id, "clock1");
        assert_eq!(patch.modules[0].module_type, "clock");
    }
}
