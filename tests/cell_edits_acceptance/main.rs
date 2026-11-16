//! Acceptance tests for editing a cell sequencer's stored cells one cell at
//! a time, from the spec alone, through public surfaces only: the module
//! description, a whole invention on the live engine through a capturing
//! backend (`set_control`, `get_control`, `apply_edits`, `document`, saving),
//! and the rendered output. By file: the declaration (`declaration`), one
//! cell's write and its refusals (`one_cell`), the copy of an asset bank into
//! the piece (`document`), and what is heard live (`live`).

mod declaration;
mod document;
mod live;
mod one_cell;

use std::path::Path;
use std::sync::{Arc, Mutex};

use fugue::rpc::{ApplyEditsReport, StructuralEdit};
use fugue::{AudioBackend, ControlValue, Invention, InventionBuilder, RpcError, RunningInvention};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// A backend that renders only when the test asks it to.
struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

/// A running piece, rendered by hand in 64-frame blocks.
pub struct Piece {
    pub running: RunningInvention,
    slot: Arc<Mutex<Option<Render>>>,
}

impl Piece {
    pub fn from_json(doc: &Value) -> Piece {
        Piece::start(serde_json::from_value(doc.clone()).expect("parses"))
    }

    pub fn from_file(path: &Path) -> Piece {
        Piece::start(Invention::from_file(path.to_str().unwrap()).expect("loads"))
    }

    fn start(invention: Invention) -> Piece {
        let (runtime, _) = InventionBuilder::new(RATE)
            .build(invention)
            .expect("builds");
        let slot = Arc::new(Mutex::new(None));
        let running = runtime
            .start_with_backend(Capture(slot.clone()))
            .expect("starts");
        Piece { running, slot }
    }

    /// Renders `blocks` blocks; returns the left and right channels.
    pub fn render(&self, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        let mut slot = self.slot.lock().unwrap();
        let render = slot.as_mut().expect("backend started");
        let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for _ in 0..blocks {
            render(&mut left, &mut right);
            l.extend_from_slice(&left);
            r.extend_from_slice(&right);
        }
        (l, r)
    }

    /// A standalone authored `set_control` of JSON text.
    pub fn write(&self, module: &str, key: &str, value: &Value) -> Result<(), String> {
        let text = ControlValue::String(value.to_string());
        (self.running.set_control(module, key, text)).map_err(|e| e.to_string())
    }

    /// An `apply_edits` batch of one `set_control` of JSON text.
    pub fn edit(
        &mut self,
        module: &str,
        key: &str,
        value: &Value,
    ) -> Result<ApplyEditsReport, RpcError> {
        self.running.apply_edits(&[StructuralEdit::SetControl {
            module_id: module.into(),
            key: key.into(),
            value: ControlValue::String(value.to_string()),
        }])
    }

    /// A control, read once the audio thread has had a block to take what
    /// was written: JSON text parsed, a number as a JSON number.
    pub fn read(&self, module: &str, key: &str) -> Value {
        self.render(1);
        match self.running.get_control(module, key) {
            Ok(ControlValue::String(text)) => serde_json::from_str(&text).unwrap(),
            Ok(ControlValue::Number(n)) => json!(n),
            other => panic!("{module}.{key}: {other:?}"),
        }
    }

    /// The retained authored document, as JSON.
    pub fn document(&self) -> Value {
        self.render(2);
        serde_json::to_value(self.running.document().expect("document retained")).unwrap()
    }
}

/// One cell: a note offset per step, `None` a rest.
pub fn cell(notes: &[Option<i32>]) -> Value {
    Value::Array(
        (notes.iter())
            .map(|n| n.map_or(Value::Null, |n| json!({ "note": n })))
            .collect(),
    )
}

/// A cell of notes on every step.
pub fn notes(offsets: &[i32]) -> Value {
    cell(&offsets.iter().map(|n| Some(*n)).collect::<Vec<_>>())
}

/// The bank most tests play: three cells of eight steps.
pub fn three_cells() -> Value {
    json!([
        notes(&[0, 2, 4, 5, 7, 9, 11, 12]),
        notes(&[12, 11, 9, 7, 5, 4, 2, 0]),
        cell(&[Some(0), None, Some(7), None, Some(0), None, Some(7), None]),
    ])
}

/// `bank` with cell `index` replaced.
pub fn with_cell(mut bank: Value, index: usize, replacement: &Value) -> Value {
    bank[index] = replacement.clone();
    bank
}

/// A module's config value for `key` in a document.
pub fn config<'a>(document: &'a Value, module: &str, key: &str) -> &'a Value {
    let modules = document["modules"].as_array().expect("modules");
    let spec = modules
        .iter()
        .find(|m| m["id"] == module)
        .expect("the module");
    &spec["config"][key]
}

/// The first and last frames two renders differ at, if any.
pub fn differences(a: &[f32], b: &[f32]) -> Option<(usize, usize)> {
    assert_eq!(a.len(), b.len());
    let differs = |(x, y): (&f32, &f32)| x.to_bits() != y.to_bits();
    let first = a.iter().zip(b).position(differs)?;
    Some((first, a.iter().zip(b).rposition(differs).unwrap()))
}

pub fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// A clock (sixteenths at 240 bpm: 3000 frames a step) driving cell
/// sequencer `seq` (with `seq_config`) into a sine voice on the left; the
/// sequencer's `right` output, scaled down, is the right channel.
pub fn voice_piece(seq_config: Value, right: &str) -> Value {
    let level = if right == "frequency" { 0.001 } else { 0.01 };
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 240 } },
            { "id": "seq", "type": "cell_sequencer", "config": seq_config },
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine" } },
            { "id": "env", "type": "adsr",
              "config": { "attack": 0.005, "decay": 0.05, "sustain": 0.6, "release": 0.03 } },
            { "id": "vca", "type": "vca" },
            { "id": "tap", "type": "vca", "config": { "level": level } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [
            wire("clock", "beat_x4", "seq", "clock"),
            wire("seq", "frequency", "osc", "frequency"),
            wire("seq", "gate", "env", "gate"),
            wire("osc", "audio", "vca", "audio"),
            wire("env", "envelope", "vca", "level"),
            wire("vca", "audio", "dac", "audio_left"),
            wire("seq", right, "tap", "audio"),
            wire("tap", "audio", "dac", "audio_right"),
        ],
    })
}

/// A looping sequencer playing `bank` from cell 0, eight steps a cycle.
pub fn looping(bank: Value) -> Value {
    json!({ "root_note": 60, "step_count": 8, "gate_length": 0.5, "start_cell": 0,
            "wait_for_cycle_end": true, "cells": bank })
}
