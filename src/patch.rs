use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A patch document defines a modular synthesis setup declaratively
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    /// Version of the patch format
    #[serde(default = "default_version")]
    pub version: String,

    /// Optional title/name for the patch
    #[serde(default)]
    pub title: Option<String>,

    /// Optional description
    #[serde(default)]
    pub description: Option<String>,

    /// The modules in the patch
    pub modules: Vec<ModuleSpec>,

    /// Connections between modules (patch cables)
    pub connections: Vec<Connection>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Patch {
    /// Load a patch from a JSON string
    pub fn from_json(json: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(json)?)
    }

    /// Load a patch from a JSON file
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
    }

    /// Convert patch to JSON string
    pub fn to_json(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// A module specification in the patch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSpec {
    /// Unique identifier for this module instance
    pub id: String,

    /// Type of module (clock, melody, voice, etc.)
    #[serde(rename = "type")]
    pub module_type: String,

    /// Module-specific configuration
    #[serde(default)]
    pub config: ModuleConfig,
}

/// Configuration for a module
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModuleConfig {
    // Clock configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<TimeSignature>,

    // Scale/Melody configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_note: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_degrees: Option<Vec<usize>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_weights: Option<Vec<f32>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_duration: Option<f32>,

    // Voice/Oscillator configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oscillator_type: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub fm_amount: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub am_amount: Option<f32>,

    // Filter configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub resonance: Option<f32>,

    // Allow arbitrary additional fields for extensibility
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Time signature [numerator, denominator]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeSignature {
    pub beats_per_measure: u32,
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

/// A connection between two modules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// Source module ID
    pub from: String,

    /// Destination module ID
    pub to: String,

    /// Optional output port name (for modules with multiple outputs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_port: Option<String>,

    /// Optional input port name (for modules with multiple inputs)
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
