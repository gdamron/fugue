//! Polyphonic development voice pool.
//!
//! A development instantiated with `config: {"voices": N}` (N > 1) becomes a
//! pre-sized pool of N identical mono instances behind the same external
//! ports. The input named `gate` drives note allocation: each rising edge
//! claims the pool voice least recently allocated (a round-robin cursor —
//! with every voice busy this is classic steal-oldest), and the falling edge
//! gates off the most recently allocated note. Other inputs follow their
//! [`DevelopmentInputMode`]: `latch` (default) samples the input at note-on
//! and holds it for that voice's note — pitch and velocity stay with the
//! note while its tail rings; `broadcast` feeds the live signal to every
//! voice each block — a sustain pedal must reach already-ringing notes.
//!
//! Outputs are the sum of every pool voice. Controls fan out: a set reaches
//! all voices, a get reads the first (they only diverge if an inner module
//! mutates its own controls).
//!
//! Everything is pre-allocated at build time; the audio path is
//! allocation-free and lock-free.

use super::*;
use crate::invention::format::DevelopmentInputMode;

/// Upper bound on pool voices. Each voice is a full copy of the development's
/// internal graph, so this bounds build cost and per-block CPU.
pub(super) const MAX_VOICES: usize = 16;

/// Per-external-input routing behavior across the pool.
#[derive(Clone, Copy, PartialEq)]
enum PortRole {
    /// The note gate: edges drive voice allocation.
    Gate,
    /// Sampled at note-on, held per voice.
    Latch,
    /// Copied live to every voice each block.
    Broadcast,
}

pub(super) struct PolyDevelopmentModule {
    name: String,
    input_ports: Vec<&'static str>,
    output_ports: Vec<&'static str>,
    roles: Vec<PortRole>,
    /// Index of the `gate` input port in `input_ports`.
    gate_port: usize,
    voices: Vec<DevelopmentModule>,
    /// Per-external-input-port block buffer (fed by the parent graph).
    input_buffers: Vec<[f32; MAX_BLOCK]>,
    /// Per-external-output-port block buffer (the pool sum).
    output_buffers: Vec<[f32; MAX_BLOCK]>,
    /// Latched note values: `latched[voice][port]` (only `Latch` ports read).
    latched: Vec<Vec<f32>>,
    /// Gate level currently fed to each voice (0.0 or 1.0).
    gate_state: Vec<f32>,
    /// Round-robin allocation cursor: the next voice to claim.
    cursor: usize,
    /// Voice holding the most recent note-on (receives the gate-off).
    active: usize,
    last_gate_in: f32,
}

impl PolyDevelopmentModule {
    pub(super) fn new(
        name: &str,
        voices: Vec<DevelopmentModule>,
        definition: &Invention,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let first = voices
            .first()
            .ok_or("polyphonic development needs at least one voice")?;
        let input_ports = first.input_ports.clone();
        let output_ports = first.output_ports.clone();

        let gate_port = input_ports
            .iter()
            .position(|port| *port == "gate")
            .ok_or_else(|| {
                format!(
                    "development '{}': voices > 1 requires a 'gate' input to allocate notes",
                    name
                )
            })?;

        let roles: Vec<PortRole> = input_ports
            .iter()
            .enumerate()
            .map(|(index, port)| {
                if index == gate_port {
                    return PortRole::Gate;
                }
                let broadcast = definition.inputs.iter().any(|input| {
                    input.name == *port && input.mode == Some(DevelopmentInputMode::Broadcast)
                });
                if broadcast {
                    PortRole::Broadcast
                } else {
                    PortRole::Latch
                }
            })
            .collect();

        let voice_count = voices.len();
        let port_count = input_ports.len();
        let output_count = output_ports.len();

        Ok(Self {
            name: name.to_string(),
            input_ports,
            output_ports,
            roles,
            gate_port,
            voices,
            input_buffers: vec![[0.0; MAX_BLOCK]; port_count],
            output_buffers: vec![[0.0; MAX_BLOCK]; output_count],
            latched: vec![vec![0.0; port_count]; voice_count],
            gate_state: vec![0.0; voice_count],
            cursor: 0,
            active: 0,
            last_gate_in: 0.0,
        })
    }
}

impl Module for PolyDevelopmentModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, frames: usize) -> bool {
        // Prime every voice's input blocks from the pool state: broadcast
        // ports mirror the live external signal, latch ports hold their
        // note-on value, the gate holds its current level. Note-on/off edges
        // below overwrite tails of these blocks.
        for v in 0..self.voices.len() {
            for p in 0..self.input_ports.len() {
                match self.roles[p] {
                    PortRole::Broadcast => {
                        let src = &self.input_buffers[p];
                        self.voices[v].input_block_mut(p)[..frames]
                            .copy_from_slice(&src[..frames]);
                    }
                    PortRole::Latch => {
                        let value = self.latched[v][p];
                        self.voices[v].input_block_mut(p)[..frames].fill(value);
                    }
                    PortRole::Gate => {
                        let level = self.gate_state[v];
                        self.voices[v].input_block_mut(p)[..frames].fill(level);
                    }
                }
            }
        }

        // Scan the external gate for note edges.
        let mut last = self.last_gate_in;
        for i in 0..frames {
            let gate_in = self.input_buffers[self.gate_port][i];
            if gate_in > 0.5 && last <= 0.5 {
                // Note-on: claim the least recently allocated voice.
                let v = self.cursor;
                self.cursor = (self.cursor + 1) % self.voices.len();
                self.active = v;

                for p in 0..self.input_ports.len() {
                    if self.roles[p] == PortRole::Latch {
                        let value = self.input_buffers[p][i];
                        self.latched[v][p] = value;
                        self.voices[v].input_block_mut(p)[i..frames].fill(value);
                    }
                }

                let block = self.voices[v].input_block_mut(self.gate_port);
                if self.gate_state[v] > 0.5 {
                    // Stealing a still-gated voice: a one-sample dip so its
                    // envelope sees a fresh rising edge.
                    block[i] = 0.0;
                    if i + 1 < frames {
                        block[i + 1..frames].fill(1.0);
                    }
                } else {
                    block[i..frames].fill(1.0);
                }
                self.gate_state[v] = 1.0;
            } else if gate_in <= 0.5 && last > 0.5 {
                // Note-off: gate off the most recent note. Ringing tails on
                // other voices keep whatever their envelopes are doing.
                let v = self.active;
                self.voices[v].input_block_mut(self.gate_port)[i..frames].fill(0.0);
                self.gate_state[v] = 0.0;
            }
            last = gate_in;
        }
        self.last_gate_in = last;

        // Run the pool and sum its outputs.
        for buffer in self.output_buffers.iter_mut() {
            buffer[..frames].fill(0.0);
        }
        for v in 0..self.voices.len() {
            self.voices[v].process(frames);
            for o in 0..self.output_ports.len() {
                let src = self.voices[v].output_block(o);
                let dst = &mut self.output_buffers[o];
                for i in 0..frames {
                    dst[i] += src[i];
                }
            }
        }

        true
    }

    fn inputs(&self) -> &[&str] {
        &self.input_ports
    }

    fn outputs(&self) -> &[&str] {
        &self.output_ports
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.input_buffers[index]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.output_buffers[index]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = self
            .input_ports
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {}", port))?;
        self.input_buffers[index].fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = self
            .output_ports
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown output port: {}", port))?;
        Ok(self.output_buffers[index][0])
    }
}

/// Fans control access out across the pool: sets reach every voice, gets and
/// metadata read the first.
pub(super) struct PolyControlSurface {
    surfaces: Vec<Arc<DevelopmentControlSurface>>,
}

impl PolyControlSurface {
    pub(super) fn new(surfaces: Vec<Arc<DevelopmentControlSurface>>) -> Self {
        Self { surfaces }
    }
}

impl ControlSurface for PolyControlSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        self.surfaces
            .first()
            .map(|surface| surface.controls())
            .unwrap_or_default()
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        self.surfaces
            .first()
            .ok_or_else(|| "empty voice pool".to_string())?
            .get_control(key)
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        for surface in &self.surfaces {
            surface.set_control(key, value.clone())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::invention::builder::InventionBuilder;
    use crate::invention::format::Invention;
    use crate::Module;

    /// A poly voice development: saw osc -> vca, adsr with a long release so
    /// notes audibly ring past their gate, plus a broadcast `pedal` input.
    fn poly_invention(voices: usize) -> Invention {
        let json = format!(
            r#"{{
            "version": "1.0.0",
            "developments": [
                {{
                    "name": "ring_voice",
                    "definition": {{
                        "modules": [
                            {{ "id": "osc", "type": "oscillator", "config": {{ "oscillator_type": "sine" }} }},
                            {{ "id": "env", "type": "adsr",
                               "config": {{ "attack": 0.0, "decay": 2.0, "sustain": 0.5, "release": 0.005 }} }},
                            {{ "id": "vca", "type": "vca" }}
                        ],
                        "connections": [
                            {{ "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" }},
                            {{ "from": "env", "from_port": "envelope", "to": "vca", "to_port": "cv" }}
                        ],
                        "inputs": [
                            {{ "name": "frequency", "to": "osc", "to_port": "frequency" }},
                            {{ "name": "gate", "to": "env", "to_port": "gate" }},
                            {{ "name": "sustain", "to": "env", "to_port": "pedal", "mode": "broadcast" }}
                        ],
                        "outputs": [
                            {{ "name": "audio", "from": "vca", "from_port": "audio" }}
                        ],
                        "controls": [
                            {{ "key": "release", "module": "env", "control": "release" }}
                        ]
                    }}
                }}
            ],
            "modules": [
                {{ "id": "voice", "type": "ring_voice", "config": {{ "voices": {} }} }},
                {{ "id": "dac", "type": "dac" }}
            ],
            "connections": [
                {{ "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio_left" }}
            ]
        }}"#,
            voices
        );
        Invention::from_json(&json).unwrap()
    }

    fn build_voice(voices: usize) -> Box<dyn Module> {
        let (runtime, _handles) = InventionBuilder::new(44100)
            .build(poly_invention(voices))
            .unwrap();
        let (_, module) = runtime
            .modules
            .into_iter()
            .find(|(id, _)| id == "voice")
            .unwrap();
        match module {
            crate::factory::GraphModule::Module(module) => module,
            _ => panic!("expected a plain module"),
        }
    }

    /// RMS of the module's audio output over `blocks` blocks of 256 frames.
    fn rms(module: &mut Box<dyn Module>, blocks: usize) -> f32 {
        let mut sum = 0.0f64;
        let mut count = 0usize;
        for _ in 0..blocks {
            module.process(256);
            let out = module.output_block(0);
            for &sample in &out[..256] {
                sum += f64::from(sample) * f64::from(sample);
                count += 1;
            }
        }
        ((sum / count as f64) as f32).sqrt()
    }

    /// Strikes a note: gate on for `on_blocks`, then off. Frequency is set
    /// before the strike so the rising edge latches it.
    fn strike(module: &mut Box<dyn Module>, frequency: f32, on_blocks: usize) {
        module.set_input("frequency", frequency).unwrap();
        module.set_input("gate", 1.0).unwrap();
        for _ in 0..on_blocks {
            module.process(256);
        }
        module.set_input("gate", 0.0).unwrap();
        module.process(256);
    }

    #[test]
    fn overlapping_notes_both_ring_with_pedal() {
        let mut module = build_voice(4);
        module.set_input("sustain", 1.0).unwrap();

        // Two notes struck in sequence, both released. With the pedal down
        // and 2s of natural decay, both tails should still be audible.
        strike(&mut module, 220.0, 4);
        strike(&mut module, 330.0, 4);

        // ~0.5s later (release is 5ms — without the pedal this is silence)
        // the pool still carries both ringing tails.
        let level = rms(&mut module, 80);
        assert!(
            level > 0.05,
            "pedal-held tails should still sound, rms = {}",
            level
        );

        // Pedal up: everything releases together.
        module.set_input("sustain", 0.0).unwrap();
        for _ in 0..40 {
            module.process(256);
        }
        let silent = rms(&mut module, 10);
        assert!(
            silent < 1e-4,
            "pedal up should release every ringing note, rms = {}",
            silent
        );
    }

    #[test]
    fn without_pedal_notes_release_normally() {
        let mut module = build_voice(4);

        strike(&mut module, 220.0, 4);
        strike(&mut module, 330.0, 4);

        // 5ms release: half a second later the pool is silent.
        for _ in 0..80 {
            module.process(256);
        }
        let level = rms(&mut module, 10);
        assert!(level < 1e-4, "unpedaled notes should be gone, rms = {}", level);
    }

    #[test]
    fn steal_oldest_keeps_recent_notes() {
        let mut module = build_voice(2);
        module.set_input("sustain", 1.0).unwrap();

        // Three overlapping notes through a two-voice pool: the first is
        // stolen, the last two keep ringing.
        strike(&mut module, 220.0, 2);
        strike(&mut module, 330.0, 2);
        strike(&mut module, 440.0, 2);

        let level = rms(&mut module, 40);
        assert!(level > 0.05, "recent notes should ring, rms = {}", level);
    }

    #[test]
    fn mono_development_still_builds() {
        // voices: 1 must take the plain mono path.
        let mut module = build_voice(1);
        strike(&mut module, 220.0, 2);
        module.process(256);
    }

    #[test]
    fn rejects_out_of_range_voices() {
        let result = InventionBuilder::new(44100).build(poly_invention(999));
        assert!(result.is_err());
    }
}
