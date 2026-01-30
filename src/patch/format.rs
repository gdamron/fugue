//! Declarative patch file format for defining synthesis setups.
//!
//! Patches are JSON documents that describe modules and their connections.

use serde::{Deserialize, Serialize};

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

    /// Type of module (e.g., "clock", "melody", "oscillator", "adsr", "vca", "dac").
    #[serde(rename = "type")]
    pub module_type: String,

    /// Module-specific configuration as generic JSON.
    ///
    /// Each module factory knows how to parse its own configuration.
    /// This allows modules to define their own config structure without
    /// requiring changes to the patch format.
    #[serde(default)]
    pub config: serde_json::Value,
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

    /// Output port name on the source module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_port: Option<String>,

    /// Input port name on the destination module.
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

        // Config is now generic JSON
        assert_eq!(patch.modules[0].config["bpm"], 120.0);
    }

    #[test]
    fn test_parse_patch_with_empty_config() {
        let json = r#"
        {
            "modules": [
                {
                    "id": "vca1",
                    "type": "vca"
                }
            ],
            "connections": []
        }
        "#;

        let patch = Patch::from_json(json).unwrap();
        assert_eq!(patch.modules[0].id, "vca1");
        assert!(patch.modules[0].config.is_null());
    }
}
