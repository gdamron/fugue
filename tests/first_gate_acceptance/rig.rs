//! A clock driving a sequencer, recorded through the dac on both surfaces:
//! the clock's gate on the left channel and the sequencer's gate on the right.

use std::sync::{Arc, Mutex};

use fugue::{AudioBackend, Invention, InventionBuilder, RenderEngine};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
/// Gate lengths are checked to this many samples: step lengths at
/// fractional tempos vary by a sample.
pub const TOLERANCE: i64 = 2;
/// Steps in the pattern.
pub const STEPS: usize = 4;
/// Passes of the pattern that are recorded.
pub const PASSES: usize = 4;

#[derive(Clone, Copy, Debug)]
pub enum Module {
    Step,
    Cell,
}

#[derive(Clone, Copy, Debug)]
pub enum Surface {
    Offline,
    Live,
}

/// One piece: tempo, the clock output that drives the sequencer, the
/// sequencer's own gate length and an optional gate length for each step.
#[derive(Clone, Debug)]
pub struct Case {
    pub bpm: f64,
    pub port: &'static str,
    pub gate_length: f64,
    pub per_step: Option<[f64; STEPS]>,
}

impl Case {
    pub fn plain(bpm: f64, port: &'static str, gate_length: f64) -> Self {
        Case {
            bpm,
            port,
            gate_length,
            per_step: None,
        }
    }

    pub fn per_step(bpm: f64, port: &'static str, default: f64, lengths: [f64; STEPS]) -> Self {
        Case {
            bpm,
            port,
            gate_length: default,
            per_step: Some(lengths),
        }
    }

    /// The gate length (a fraction of the step) step `i` of the pattern has.
    pub fn length_of(&self, i: usize) -> f64 {
        self.per_step.map_or(self.gate_length, |l| l[i % STEPS])
    }

    fn document(&self, module: Module) -> Value {
        let steps: Vec<Value> = (0..STEPS)
            .map(|i| {
                let mut step = json!({ "note": (i as i64) * 2 });
                if self.per_step.is_some() {
                    step["gate_length"] = json!(self.length_of(i));
                }
                step
            })
            .collect();
        let (kind, config) = match module {
            Module::Step => (
                "step_sequencer",
                json!({
                    "root_note": 60, "step_count": STEPS,
                    "gate_length": self.gate_length, "pattern": steps
                }),
            ),
            Module::Cell => (
                "cell_sequencer",
                json!({
                    "root_note": 60, "step_count": STEPS,
                    "gate_length": self.gate_length, "cells": [steps]
                }),
            ),
        };
        json!({
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": self.bpm } },
                { "id": "seq", "type": kind, "config": config },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "clock", "from_port": self.port, "to": "seq", "to_port": "clock" },
                { "from": "clock", "from_port": self.port, "to": "dac", "to_port": "audio_left" },
                { "from": "seq", "from_port": "gate", "to": "dac", "to_port": "audio_right" }
            ]
        })
    }

    /// Samples in one step at this tempo, on average.
    fn step_samples(&self) -> f64 {
        let per_beat = match self.port {
            "beat" => 1.0,
            "beat_x2" => 2.0,
            "beat_x4" => 4.0,
            other => panic!("unsupported clock port {other}"),
        };
        RATE as f64 * 60.0 / (self.bpm * per_beat)
    }

    /// Frames to record: every pass, plus a step and a half so the last
    /// step's length is measured by the edge that ends it.
    fn frames(&self) -> usize {
        (self.step_samples() * (STEPS * PASSES) as f64 * 1.0 + self.step_samples() * 2.5) as usize
    }
}

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

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

/// What the dac sent: the clock's gate and the sequencer's gate.
pub struct Recording {
    pub clock: Vec<f32>,
    pub gate: Vec<f32>,
}

pub fn record(case: &Case, module: Module, surface: Surface) -> Recording {
    let document = case.document(module);
    let frames = case.frames();
    match surface {
        Surface::Offline => {
            let mut engine = RenderEngine::new(RATE);
            engine.load_json(&document.to_string()).expect("loads");
            let mut out = vec![0.0f32; frames * 2];
            engine.render_interleaved(&mut out).expect("renders");
            Recording {
                clock: out.iter().step_by(2).copied().collect(),
                gate: out.iter().skip(1).step_by(2).copied().collect(),
            }
        }
        Surface::Live => {
            let invention: Invention = serde_json::from_value(document).unwrap();
            let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
            let slot = Arc::new(Mutex::new(None));
            let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
            let (mut clock, mut gate) = (Vec::new(), Vec::new());
            {
                let mut slot = slot.lock().unwrap();
                let render = slot.as_mut().expect("backend started");
                let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
                while clock.len() < frames {
                    render(&mut l, &mut r);
                    clock.extend_from_slice(&l);
                    gate.extend_from_slice(&r);
                }
            }
            running.stop();
            Recording { clock, gate }
        }
    }
}

/// Samples where `signal` rises past 0.5 (the first counts when it starts
/// high).
pub fn rises(signal: &[f32]) -> Vec<usize> {
    (0..signal.len())
        .filter(|&i| signal[i] > 0.5 && (i == 0 || signal[i - 1] <= 0.5))
        .collect()
}

/// How long the gate that rose at `at` stays high.
pub fn high_for(signal: &[f32], at: usize) -> usize {
    signal[at..].iter().take_while(|&&s| s > 0.5).count()
}

/// One step as the recording shows it.
#[derive(Debug)]
pub struct Step {
    /// The clock edge that starts the step, and the one that ends it.
    pub edge: usize,
    pub next_edge: usize,
    /// Where the sequencer's gate rose within the step, and how long it was
    /// high.
    pub rise: usize,
    pub high: usize,
}

impl Step {
    pub fn length(&self) -> usize {
        self.next_edge - self.edge
    }
}

/// Every recorded step, in order (`STEPS * PASSES` of them), or what is
/// wrong with the recording.
pub fn steps(rec: &Recording) -> Result<Vec<Step>, String> {
    let edges = rises(&rec.clock);
    let total = STEPS * PASSES;
    if edges.len() < total + 1 {
        return Err(format!("only {} clock edges recorded", edges.len()));
    }
    let gates = rises(&rec.gate);
    let mut out = Vec::new();
    for k in 0..total {
        let (edge, next_edge) = (edges[k], edges[k + 1]);
        let within: Vec<usize> = gates
            .iter()
            .copied()
            .filter(|&g| g + TOLERANCE as usize >= edge && g < next_edge)
            .collect();
        let [rise] = within[..] else {
            return Err(format!(
                "step {k}: expected one gate rise in {edge}..{next_edge}, found {within:?}"
            ));
        };
        out.push(Step {
            edge,
            next_edge,
            rise,
            high: high_for(&rec.gate, rise),
        });
    }
    Ok(out)
}
