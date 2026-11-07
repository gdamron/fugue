//! Shared documents and helpers for the minimum-core-version acceptance tests.

#![allow(dead_code)]

use fugue::Invention;
use serde_json::{json, Value};

/// The core the tests run against.
pub const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// The key a document declares its minimum core under.
pub const FIELD: &str = "minimum_core_version";

/// The core that first understands development control ranges.
pub const RANGES_SINCE: (u64, u64, u64) = (2026, 11, 0);

/// A minimum no core will reach in the lifetime of these tests.
pub const FAR_FUTURE: &str = "9999.1.0";

/// `"YYYY.M.P"` as numbers, or `None` when it is not three whole numbers.
pub fn version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.').map(|p| p.parse::<u64>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(triple)
}

pub fn running() -> (u64, u64, u64) {
    version(RUNNING).expect("the crate version is YYYY.M.P")
}

// -- documents -------------------------------------------------------------------

fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// A sine oscillator into the dac, with no versioned feature.
pub fn plain() -> Value {
    json!({
        "version": "1.0.0",
        "title": "plain",
        "modules": [
            { "id": "osc", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": 220.0 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [ wire("osc", "audio", "dac", "audio") ],
    })
}

/// A development: an lfo `wobble` whose `rate` is exposed as `rate`, with a
/// `range` narrowing it when `ranged`.
pub fn kit_definition(ranged: bool) -> Value {
    let mut rate = json!({ "name": "rate", "module": "wobble", "control": "rate" });
    if ranged {
        rate["range"] = json!({ "min": 0.5, "max": 8.0 });
    }
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "wobble", "type": "lfo", "config": { "rate": 2.0, "waveform": "sine" } },
            { "id": "src", "type": "oscillator",
              "config": { "waveform": "square", "frequency": 0.001 } },
            { "id": "mix", "type": "mixer", "config": { "channel_count": 1 } },
        ],
        "connections": [ wire("src", "audio", "mix", "audio.0") ],
        "outputs": [ { "name": "audio", "from": "mix", "from_port": "audio_left" } ],
        "controls": [ rate ],
    })
}

/// A main graph holding one instance of the development `kit`, defined inline.
pub fn kit_document(definition: Value) -> Value {
    json!({
        "version": "1.0.0",
        "developments": [ { "name": "kit", "definition": definition } ],
        "modules": [
            { "id": "v", "type": "kit", "config": {} },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [ wire("v", "audio", "dac", "audio") ],
    })
}

/// `doc` declaring `minimum` at its top level.
pub fn declaring(mut doc: Value, minimum: impl Into<Value>) -> Value {
    doc[FIELD] = minimum.into();
    doc
}

/// `doc` with the field removed at the top level and in inline definitions.
pub fn without_field(mut doc: Value) -> Value {
    if let Some(object) = doc.as_object_mut() {
        object.remove(FIELD);
    }
    if let Some(specs) = doc.get_mut("developments").and_then(Value::as_array_mut) {
        for spec in specs {
            if let Some(definition) = spec.get_mut("definition") {
                *definition = without_field(definition.take());
            }
        }
    }
    doc
}

pub fn parse(doc: &Value) -> Invention {
    Invention::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("parses: {e}\n{doc:#}"))
}

/// The document as the core serializes it, minus the field.
pub fn normalized(doc: &Value) -> Value {
    let json = parse(doc).to_json().expect("serializes");
    without_field(serde_json::from_str(&json).expect("is JSON"))
}
