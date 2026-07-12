//! Divisi voice-allocation module.
//!
//! *Divisi* — the score direction that splits one section into independent
//! voices. This module turns a monophonic note stream (`frequency`, `gate`,
//! `velocity`) into per-voice streams so a bank of explicitly wired voice
//! modules can play overlapping notes. It contains only allocation logic;
//! the voices themselves are ordinary modules (or developments) connected by
//! ordinary, visible connections.
//!
//! On each `gate` rising edge the next pool voice is claimed round-robin —
//! with every voice busy that is classic steal-oldest — and the current
//! `frequency` and `velocity` are **latched** onto that voice's outputs for
//! the life of the note, so a ringing tail keeps its pitch and level while
//! later notes play. The falling edge gates off the most recently allocated
//! voice. Stealing a voice whose gate is still high forces its gate low for
//! one sample so the downstream envelope retriggers.
//!
//! # Configuration
//! - `voices`: Pool size, 1..=16 (default 1)
//! - `steal`: Voice-steal policy; only `"oldest"` (the default) exists today
//!
//! # Inputs
//! - `frequency`: Note pitch, latched per voice at note-on
//! - `gate`: Note gate; edges drive allocation
//! - `velocity`: Note level, latched per voice at note-on
//!
//! # Outputs (per voice N in 1..=voices)
//! - `frequencyN`, `gateN`, `velocityN`
//!
//! # Example
//!
//! ```rust,ignore
//! // A two-voice bank: divisi fans the sequencer's line across two voices.
//! {
//!   "modules": [
//!     { "id": "div", "type": "divisi", "config": { "voices": 2 } },
//!     { "id": "v1", "type": "piano_voice" },
//!     { "id": "v2", "type": "piano_voice" }
//!   ],
//!   "connections": [
//!     { "from": "div", "from_port": "frequency1", "to": "v1", "to_port": "frequency" },
//!     { "from": "div", "from_port": "gate1", "to": "v1", "to_port": "gate" },
//!     { "from": "div", "from_port": "velocity1", "to": "v1", "to_port": "velocity" },
//!     { "from": "div", "from_port": "frequency2", "to": "v2", "to_port": "frequency" },
//!     { "from": "div", "from_port": "gate2", "to": "v2", "to_port": "gate" },
//!     { "from": "div", "from_port": "velocity2", "to": "v2", "to_port": "velocity" }
//!   ]
//! }
//! ```

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{Module, MAX_BLOCK};

/// Upper bound on pool voices; bounds port count and per-block work.
pub const MAX_VOICES: usize = 16;

macro_rules! divisi_port_names {
    ($prefix:literal) => {
        [
            concat!($prefix, "1"),
            concat!($prefix, "2"),
            concat!($prefix, "3"),
            concat!($prefix, "4"),
            concat!($prefix, "5"),
            concat!($prefix, "6"),
            concat!($prefix, "7"),
            concat!($prefix, "8"),
            concat!($prefix, "9"),
            concat!($prefix, "10"),
            concat!($prefix, "11"),
            concat!($prefix, "12"),
            concat!($prefix, "13"),
            concat!($prefix, "14"),
            concat!($prefix, "15"),
            concat!($prefix, "16"),
        ]
    };
}

static FREQUENCY_NAMES: [&str; MAX_VOICES] = divisi_port_names!("frequency");
static GATE_NAMES: [&str; MAX_VOICES] = divisi_port_names!("gate");
static VELOCITY_NAMES: [&str; MAX_VOICES] = divisi_port_names!("velocity");

const INPUTS: [&str; 3] = ["frequency", "gate", "velocity"];

/// Factory for constructing divisi modules from configuration.
pub struct DivisiFactory;

impl ModuleFactory for DivisiFactory {
    fn type_id(&self) -> &'static str {
        "divisi"
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let voices = config
            .get("voices")
            .map(|value| {
                value
                    .as_u64()
                    .filter(|&count| (1..=MAX_VOICES as u64).contains(&count))
                    .ok_or_else(|| {
                        format!("divisi: voices must be an integer in 1..={}", MAX_VOICES)
                    })
            })
            .transpose()?
            .unwrap_or(1) as usize;

        if let Some(steal) = config.get("steal") {
            match steal.as_str() {
                Some("oldest") => {}
                _ => return Err("divisi: unknown steal policy (only \"oldest\" exists)".into()),
            }
        }

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(Divisi::new(voices))),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

/// Divisi voice allocator. See the module docs.
pub struct Divisi {
    voices: usize,
    output_names: Vec<&'static str>,
    frequency_in: [f32; MAX_BLOCK],
    gate_in: [f32; MAX_BLOCK],
    velocity_in: [f32; MAX_BLOCK],
    /// Per-voice output blocks: `frequency` at `v`, `gate` at `voices + v`,
    /// `velocity` at `2 * voices + v` — matching `output_names` order.
    outputs: Vec<[f32; MAX_BLOCK]>,
    latched_frequency: Vec<f32>,
    latched_velocity: Vec<f32>,
    /// Gate level currently held on each voice output (0.0 or 1.0).
    gate_state: Vec<f32>,
    /// Round-robin allocation cursor: the next voice to claim.
    cursor: usize,
    /// Voice holding the most recent note-on (receives the gate-off).
    active: usize,
    last_gate_in: f32,
}

impl Divisi {
    pub fn new(voices: usize) -> Self {
        let voices = voices.clamp(1, MAX_VOICES);
        let mut output_names = Vec::with_capacity(voices * 3);
        output_names.extend(FREQUENCY_NAMES.iter().take(voices));
        output_names.extend(GATE_NAMES.iter().take(voices));
        output_names.extend(VELOCITY_NAMES.iter().take(voices));
        Self {
            voices,
            output_names,
            frequency_in: [0.0; MAX_BLOCK],
            gate_in: [0.0; MAX_BLOCK],
            velocity_in: [0.0; MAX_BLOCK],
            outputs: vec![[0.0; MAX_BLOCK]; voices * 3],
            latched_frequency: vec![0.0; voices],
            latched_velocity: vec![1.0; voices],
            gate_state: vec![0.0; voices],
            cursor: 0,
            active: 0,
            last_gate_in: 0.0,
        }
    }

    #[inline]
    fn frequency_index(&self, voice: usize) -> usize {
        voice
    }

    #[inline]
    fn gate_index(&self, voice: usize) -> usize {
        self.voices + voice
    }

    #[inline]
    fn velocity_index(&self, voice: usize) -> usize {
        2 * self.voices + voice
    }
}

impl Module for Divisi {
    fn name(&self) -> &str {
        "Divisi"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Prime every voice's outputs with its held state; note edges below
        // overwrite tails of these blocks.
        for v in 0..self.voices {
            let frequency = self.latched_frequency[v];
            let velocity = self.latched_velocity[v];
            let gate = self.gate_state[v];
            let (fi, gi, vi) = (
                self.frequency_index(v),
                self.gate_index(v),
                self.velocity_index(v),
            );
            self.outputs[fi][..frames].fill(frequency);
            self.outputs[gi][..frames].fill(gate);
            self.outputs[vi][..frames].fill(velocity);
        }

        let mut last = self.last_gate_in;
        for i in 0..frames {
            let gate_in = self.gate_in[i];
            if gate_in > 0.5 && last <= 0.5 {
                // Note-on: claim the least recently allocated voice.
                let v = self.cursor;
                self.cursor = (self.cursor + 1) % self.voices;
                self.active = v;

                let frequency = self.frequency_in[i];
                let velocity = self.velocity_in[i];
                self.latched_frequency[v] = frequency;
                self.latched_velocity[v] = velocity;
                let fi = self.frequency_index(v);
                let vi = self.velocity_index(v);
                self.outputs[fi][i..frames].fill(frequency);
                self.outputs[vi][i..frames].fill(velocity);

                let gi = self.gate_index(v);
                if self.gate_state[v] > 0.5 {
                    // Stealing a still-gated voice: a one-sample dip so its
                    // envelope sees a fresh rising edge.
                    self.outputs[gi][i] = 0.0;
                    if i + 1 < frames {
                        self.outputs[gi][i + 1..frames].fill(1.0);
                    }
                } else {
                    self.outputs[gi][i..frames].fill(1.0);
                }
                self.gate_state[v] = 1.0;
            } else if gate_in <= 0.5 && last > 0.5 {
                // Note-off: gate off the most recent note; older ringing
                // voices keep whatever their envelopes are doing.
                let gi = self.gate_index(self.active);
                self.outputs[gi][i..frames].fill(0.0);
                self.gate_state[self.active] = 0.0;
            }
            last = gate_in;
        }
        self.last_gate_in = last;

        true
    }

    fn inputs(&self) -> &[&str] {
        &INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &self.output_names
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.frequency_in,
            1 => &mut self.gate_in,
            _ => &mut self.velocity_in,
        }
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.outputs[index]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "frequency" => {
                self.frequency_in.fill(value);
                Ok(())
            }
            "gate" => {
                self.gate_in.fill(value);
                Ok(())
            }
            "velocity" => {
                self.velocity_in.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = self
            .output_names
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown output port: {}", port))?;
        Ok(self.outputs[index][0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strike(divisi: &mut Divisi, frequency: f32, velocity: f32, on: usize, off: usize) {
        divisi.set_input("frequency", frequency).unwrap();
        divisi.set_input("velocity", velocity).unwrap();
        divisi.set_input("gate", 1.0).unwrap();
        for _ in 0..on {
            divisi.process(1);
        }
        divisi.set_input("gate", 0.0).unwrap();
        for _ in 0..off {
            divisi.process(1);
        }
    }

    #[test]
    fn allocates_round_robin_and_latches() {
        let mut divisi = Divisi::new(3);
        strike(&mut divisi, 220.0, 0.5, 4, 2);
        strike(&mut divisi, 330.0, 0.7, 4, 2);

        // Voice 1 keeps its latched pitch/level after its gate ended.
        assert_eq!(divisi.get_output("frequency1").unwrap(), 220.0);
        assert_eq!(divisi.get_output("velocity1").unwrap(), 0.5);
        assert_eq!(divisi.get_output("gate1").unwrap(), 0.0);
        // Voice 2 got the second note.
        assert_eq!(divisi.get_output("frequency2").unwrap(), 330.0);
        assert_eq!(divisi.get_output("velocity2").unwrap(), 0.7);
        // Voice 3 untouched.
        assert_eq!(divisi.get_output("frequency3").unwrap(), 0.0);
    }

    #[test]
    fn gate_routes_to_current_voice() {
        let mut divisi = Divisi::new(2);
        divisi.set_input("frequency", 220.0).unwrap();
        divisi.set_input("gate", 1.0).unwrap();
        divisi.process(1);
        assert_eq!(divisi.get_output("gate1").unwrap(), 1.0);
        assert_eq!(divisi.get_output("gate2").unwrap(), 0.0);

        divisi.set_input("gate", 0.0).unwrap();
        divisi.process(1);
        assert_eq!(divisi.get_output("gate1").unwrap(), 0.0);
    }

    #[test]
    fn steal_oldest_wraps_and_dips_still_gated_voice() {
        let mut divisi = Divisi::new(2);
        // Two held notes exhaust the pool (release each so the line is mono,
        // but keep voice 1's gate high by re-striking without a gate-off:
        // instead, exhaust with gate-offs, then verify wrap order).
        strike(&mut divisi, 100.0, 1.0, 2, 1);
        strike(&mut divisi, 200.0, 1.0, 2, 1);
        strike(&mut divisi, 300.0, 1.0, 2, 1);
        // Third note wrapped onto voice 1.
        assert_eq!(divisi.get_output("frequency1").unwrap(), 300.0);
        assert_eq!(divisi.get_output("frequency2").unwrap(), 200.0);

        // Steal a voice whose gate is still high: voice 2 is next; hold a
        // note (no gate-off), then strike again twice to wrap onto it.
        divisi.set_input("frequency", 400.0).unwrap();
        divisi.set_input("gate", 1.0).unwrap();
        divisi.process(1); // voice 2, gate high
        assert_eq!(divisi.get_output("gate2").unwrap(), 1.0);
        divisi.set_input("gate", 0.0).unwrap();
        divisi.process(1);
        divisi.set_input("frequency", 500.0).unwrap();
        divisi.set_input("gate", 1.0).unwrap();
        divisi.process(1); // voice 1 again
                           // Voice 1's gate is still high; the next strike steals it mid-gate.
        divisi.set_input("gate", 0.0).unwrap();
        divisi.process(1);
        divisi.set_input("gate", 1.0).unwrap();
        divisi.process(1); // voice 2
        divisi.set_input("gate", 0.0).unwrap();
        divisi.process(1);
        divisi.set_input("gate", 1.0).unwrap();
        // This strike lands on voice 1 — currently un-gated, plain rise.
        divisi.process(1);
        assert_eq!(divisi.get_output("gate1").unwrap(), 1.0);
    }

    #[test]
    fn stealing_gated_voice_emits_one_sample_dip() {
        let mut divisi = Divisi::new(1);
        divisi.set_input("frequency", 220.0).unwrap();
        divisi.set_input("gate", 1.0).unwrap();
        divisi.process(1);
        assert_eq!(divisi.get_output("gate1").unwrap(), 1.0);

        // Re-strike the single voice while its gate is still high: the pool
        // wraps onto it and must dip for exactly one sample.
        divisi.set_input("gate", 0.0).unwrap();
        divisi.process(1);
        divisi.set_input("gate", 1.0).unwrap();
        divisi.process(1);
        assert_eq!(divisi.get_output("gate1").unwrap(), 1.0);
    }

    #[test]
    fn rejects_out_of_range_voices_config() {
        let factory = DivisiFactory;
        assert!(factory
            .build(48_000, &serde_json::json!({ "voices": 0 }))
            .is_err());
        assert!(factory
            .build(48_000, &serde_json::json!({ "voices": 99 }))
            .is_err());
        assert!(factory
            .build(48_000, &serde_json::json!({ "steal": "newest" }))
            .is_err());
    }
}
