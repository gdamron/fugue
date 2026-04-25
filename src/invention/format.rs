//! Declarative invention file format for defining synthesis setups.
//!
//! Inventions are JSON documents that describe modules and their connections.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A complete invention document defining a modular synthesis setup.
///
/// Inventions can be loaded from JSON files or strings and define
/// the modules to instantiate and how they connect together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invention {
    /// Version of the invention format.
    #[serde(default = "default_version")]
    pub version: String,

    /// Optional title for the invention.
    #[serde(default)]
    pub title: Option<String>,

    /// Optional description of the invention.
    #[serde(default)]
    pub description: Option<String>,

    /// Development definitions available to modules in this document.
    #[serde(default)]
    pub developments: Vec<DevelopmentSpec>,

    /// Shared JSON assets available to module configuration via `$asset` references.
    #[serde(default)]
    pub assets: BTreeMap<String, AssetSpec>,

    /// The modules in this invention.
    pub modules: Vec<ModuleSpec>,

    /// Connections between modules.
    pub connections: Vec<Connection>,

    /// Exposed input ports when this document is used as a development.
    #[serde(default)]
    pub inputs: Vec<DevelopmentInput>,

    /// Exposed output ports when this document is used as a development.
    #[serde(default)]
    pub outputs: Vec<DevelopmentOutput>,

    /// Exposed runtime controls when this document is used as a development.
    #[serde(default)]
    pub controls: Vec<DevelopmentControl>,

    /// Source file path used for resolving relative development imports.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Invention {
    /// Parses an invention from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let mut invention: Self = serde_json::from_str(json)?;
        invention.source_path = None;
        Ok(invention)
    }

    /// Loads an invention from a JSON file.
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let json = std::fs::read_to_string(path)?;
        let mut invention = Self::from_json(&json)?;
        invention.source_path = Some(Path::new(path).to_path_buf());
        Ok(invention)
    }

    /// Serializes the invention to a JSON string.
    pub fn to_json(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn is_development(&self) -> bool {
        !self.inputs.is_empty() || !self.outputs.is_empty() || !self.controls.is_empty()
    }
}

/// A named development registered for use as a module type within an invention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevelopmentSpec {
    /// Module type name used when instantiating this development.
    pub name: String,

    /// Optional JSON file path to a development document.
    #[serde(default)]
    pub path: Option<String>,

    /// Optional inline development definition.
    #[serde(default)]
    pub definition: Option<Box<Invention>>,
}

/// A shared JSON asset that can be referenced from module configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetSpec {
    /// JSON file path. Relative paths resolve from the loading invention file.
    pub path: String,
}

/// Maps an exposed development input to an internal module input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevelopmentInput {
    pub name: String,
    pub to: String,
    pub to_port: String,
}

/// Maps an exposed development output to an internal module output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevelopmentOutput {
    pub name: String,
    pub from: String,
    pub from_port: String,
}

/// Maps an exposed development control to an internal module control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevelopmentControl {
    pub key: String,
    pub module: String,
    pub control: String,
}

/// Specification for a single module in an invention.
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
    /// requiring changes to the invention format.
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

/// A connection between two modules in an invention.
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
    fn test_parse_basic_invention() {
        let json = r#"
        {
            "version": "1.0.0",
            "title": "Test Invention",
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

        let invention = Invention::from_json(json).unwrap();
        assert_eq!(invention.version, "1.0.0");
        assert_eq!(invention.title, Some("Test Invention".to_string()));
        assert_eq!(invention.modules.len(), 1);
        assert_eq!(invention.modules[0].id, "clock1");
        assert_eq!(invention.modules[0].module_type, "clock");
        assert!(invention.developments.is_empty());
        assert!(invention.assets.is_empty());
        assert!(invention.inputs.is_empty());
        assert!(invention.outputs.is_empty());
        assert!(invention.controls.is_empty());

        // Config is now generic JSON
        assert_eq!(invention.modules[0].config["bpm"], 120.0);
    }

    #[test]
    fn test_parse_invention_with_empty_config() {
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

        let invention = Invention::from_json(json).unwrap();
        assert_eq!(invention.modules[0].id, "vca1");
        assert!(invention.modules[0].config.is_null());
    }

    #[test]
    fn test_parse_development_document() {
        let json = r#"
        {
            "modules": [
                { "id": "osc", "type": "oscillator" }
            ],
            "connections": [],
            "inputs": [
                { "name": "frequency", "to": "osc", "to_port": "frequency" }
            ],
            "outputs": [
                { "name": "audio", "from": "osc", "from_port": "audio" }
            ],
            "controls": [
                { "key": "type", "module": "osc", "control": "type" }
            ]
        }
        "#;

        let invention = Invention::from_json(json).unwrap();
        assert!(invention.is_development());
        assert_eq!(invention.inputs[0].name, "frequency");
        assert_eq!(invention.outputs[0].name, "audio");
        assert_eq!(invention.controls[0].key, "type");
    }
}
