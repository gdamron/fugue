//! A clock driving a sequencer whose tempo changes mid-piece, recorded
//! through the dac: the clock's gate on the left channel and the
//! sequencer's gate on the right. The change is a write to the clock's `bpm`
//! control and nothing else.

use std::sync::{Arc, Mutex};

use fugue::rpc::{ControlTiming, ControlWhen};
use fugue::{
    AudioBackend, ControlValue, ControlWrite, ControlWriteIntent, Invention, InventionBuilder,
    RenderEngine,
};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
/// Gate lengths are checked to this many samples: sequencers measure a step
/// as the integer gap between clock edges.
pub const TOLERANCE: i64 = 2;
/// Steps in the pattern.
pub const PATTERN: usize = 4;
/// Steps recorded after the one the change lands in or on.
pub const AFTER: usize = 10;
const BLOCK: usize = 64;

#[derive(Clone, Copy, Debug)]
pub enum Module {
    Step,
    Cell,
}

/// How the `bpm` write reaches the clock.
#[derive(Clone, Copy, Debug)]
pub enum Delivery {
    /// Offline: render up to the sample, then `RenderEngine::set_control`.
    Offline,
    /// Live: a request timed `AtSample`.
    LiveAtSample,
    /// Live: a request timed to the clock's next bar line (4 beats a bar).
    LiveNextBar,
}

/// Where the write lands relative to the clock's steps.
#[derive(Clone, Copy, Debug)]
pub enum Landing {
    /// Exactly on the clock edge that starts step `step` (the bar line).
    OnEdge { step: usize },
    /// Three quarters of the way through step `step`, after its gate (half
    /// the step at most) has ended.
    MidStep { step: usize },
}

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub from_bpm: f64,
    pub to_bpm: f64,
    pub gate_length: f64,
    pub landing: Landing,
}

fn step_samples(bpm: f64) -> f64 {
    RATE as f64 * 60.0 / bpm
}

impl Case {
    /// The sample the write is timed for. A write on an edge lands on the
    /// clock's own rise: measured from a probe render of the same piece at
    /// the old tempo (the first beat is a sample short of the rest, so the
    /// edges are not at whole multiples of the step). A mid-step write is
    /// three quarters of the way through the step, moved down to the start
    /// of its block so it lands on the same sample offline and live.
    pub fn write_sample(&self, module: Module) -> u64 {
        match self.landing {
            Landing::OnEdge { step } => {
                let probe = Case {
                    to_bpm: self.from_bpm,
                    landing: Landing::OnEdge { step },
                    ..*self
                };
                let frames = ((step + 2) as f64 * step_samples(self.from_bpm)) as usize;
                let mut engine = RenderEngine::new(RATE);
                engine
                    .load_json(&probe.document(module).to_string())
                    .expect("loads");
                let mut out = vec![0.0f32; frames * 2];
                engine.render_interleaved(&mut out).expect("renders");
                let clock: Vec<f32> = out.iter().step_by(2).copied().collect();
                let edges = rises(&clock);
                *edges
                    .get(step)
                    .unwrap_or_else(|| panic!("{self:?}: probe has only {} edges", edges.len()))
                    as u64
            }
            Landing::MidStep { step } => {
                let at = ((step as f64 + 0.75) * step_samples(self.from_bpm)).round() as u64;
                at / BLOCK as u64 * BLOCK as u64
            }
        }
    }

    /// How many samples a gate may be off: 2, and one more when a step is
    /// not a whole number of samples, where the integer gap between edges
    /// jitters by a sample on top of the sample the gate itself is short.
    pub fn slack(&self) -> i64 {
        let whole = |bpm: f64| (step_samples(bpm) - step_samples(bpm).round()).abs() < 1e-6;
        TOLERANCE + i64::from(!whole(self.from_bpm) || !whole(self.to_bpm))
    }

    /// The step the change is in or on.
    pub fn change_step(&self) -> usize {
        match self.landing {
            Landing::OnEdge { step } | Landing::MidStep { step } => step,
        }
    }

    fn document(&self, module: Module) -> Value {
        let steps: Vec<Value> = (0..PATTERN)
            .map(|i| json!({ "note": (i as i64) * 2 }))
            .collect();
        let (kind, config) = match module {
            Module::Step => (
                "step_sequencer",
                json!({
                    "root_note": 60, "step_count": PATTERN,
                    "gate_length": self.gate_length, "pattern": steps
                }),
            ),
            Module::Cell => (
                "cell_sequencer",
                json!({
                    "root_note": 60, "step_count": PATTERN,
                    "gate_length": self.gate_length, "cells": [steps]
                }),
            ),
        };
        json!({
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": self.from_bpm } },
                { "id": "seq", "type": kind, "config": config },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
            ],
            "connections": [
                { "from": "clock", "from_port": "beat", "to": "seq", "to_port": "clock" },
                { "from": "clock", "from_port": "beat", "to": "dac", "to_port": "audio_left" },
                { "from": "seq", "from_port": "gate", "to": "dac", "to_port": "audio_right" }
            ]
        })
    }

    /// Frames to record: up to the change, then enough new-tempo steps.
    fn frames(&self, write_at: usize) -> usize {
        let before = write_at as f64 + step_samples(self.from_bpm);
        let frames = before + (AFTER + 2) as f64 * step_samples(self.to_bpm);
        (frames as usize).div_ceil(BLOCK) * BLOCK
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

fn bpm_write(case: &Case, timing: ControlTiming) -> ControlWrite {
    ControlWrite {
        intent: ControlWriteIntent::Perform,
        timing,
        ..ControlWrite::new("clock", "bpm", ControlValue::Number(case.to_bpm as f32))
    }
}

pub fn record(case: &Case, module: Module, delivery: Delivery) -> Recording {
    let document = case.document(module);
    let write_at = case.write_sample(module) as usize;
    let frames = case.frames(write_at);
    match delivery {
        Delivery::Offline => {
            let mut engine = RenderEngine::new(RATE);
            engine.load_json(&document.to_string()).expect("loads");
            let mut out = vec![0.0f32; frames * 2];
            let (head, tail) = out.split_at_mut(write_at * 2);
            engine.render_interleaved(head).expect("renders");
            engine
                .set_control("clock", "bpm", ControlValue::Number(case.to_bpm as f32))
                .expect("bpm write");
            engine.render_interleaved(tail).expect("renders");
            Recording {
                clock: out.iter().step_by(2).copied().collect(),
                gate: out.iter().skip(1).step_by(2).copied().collect(),
            }
        }
        Delivery::LiveAtSample | Delivery::LiveNextBar => {
            let invention: Invention = serde_json::from_value(document).unwrap();
            let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
            let slot = Arc::new(Mutex::new(None));
            let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
            let when = match delivery {
                Delivery::LiveAtSample => ControlWhen::AtSample {
                    sample: write_at as u64,
                },
                _ => ControlWhen::NextBar {
                    beats_per_bar: Some(4.0),
                    clock: None,
                },
            };
            let write = bpm_write(
                case,
                ControlTiming {
                    when: Some(when),
                    ttl_ms: None,
                },
            );
            // A request timed to the next bar is made half a beat into the
            // bar before it (the clock starts on a bar line); the others
            // before any audio.
            let request_at = match delivery {
                Delivery::LiveNextBar => (4.5 * step_samples(case.from_bpm)) as usize,
                _ => 0,
            };
            let mut write = Some(write);
            let (mut clock, mut gate) = (Vec::new(), Vec::new());
            {
                let mut slot = slot.lock().unwrap();
                let render = slot.as_mut().expect("backend started");
                let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
                while clock.len() < frames {
                    if clock.len() >= request_at {
                        if let Some(write) = write.take() {
                            let acks = running.write_controls(&[write]);
                            assert!(acks.error.is_none(), "{:?}", acks.error);
                        }
                    }
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

/// One step as the recording shows it.
#[derive(Debug)]
pub struct Step {
    /// The clock edge that starts the step, and the one that ends it.
    pub edge: usize,
    pub next_edge: usize,
    /// How long the step's gate is high from its edge, or what is wrong
    /// (no gate rise on the edge: a gate that ran into the next edge shows
    /// here).
    pub high: Result<usize, String>,
}

impl Step {
    pub fn length(&self) -> usize {
        self.next_edge - self.edge
    }
}

/// Every step between the first and the last clock edge recorded.
pub fn steps(rec: &Recording) -> Vec<Step> {
    let edges = rises(&rec.clock);
    let gates = rises(&rec.gate);
    (0..edges.len().saturating_sub(1))
        .map(|k| {
            let (edge, next_edge) = (edges[k], edges[k + 1]);
            let within: Vec<usize> = gates
                .iter()
                .copied()
                .filter(|&g| g + TOLERANCE as usize >= edge && g < next_edge)
                .collect();
            let high = match within[..] {
                [rise] => Ok(rec.gate[rise..].iter().take_while(|&&s| s > 0.5).count()),
                _ => Err(format!(
                    "expected one gate rise in {edge}..{next_edge}, found {within:?} \
                     (a gate still high at the next edge hides that step's rise)"
                )),
            };
            Step {
                edge,
                next_edge,
                high,
            }
        })
        .collect()
}
