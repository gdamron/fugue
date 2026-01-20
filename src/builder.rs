use crate::module::{Connect, ConnectedProcessor};
use crate::patch::{ModuleConfig, ModuleSpec, Patch};
use crate::scale::{Mode, Note, Scale};
use crate::sequencer::MelodyParams;
use crate::time::{Clock, Tempo};
use crate::{Audio, Dac, MelodyGenerator, OscillatorType, Voice};
use std::collections::HashMap;

/// Builds a runnable audio graph from a patch document
pub struct PatchBuilder {
    sample_rate: u32,
}

impl PatchBuilder {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Build and run a patch from a Patch document
    pub fn build_and_run(&self, patch: Patch) -> Result<PatchRuntime, Box<dyn std::error::Error>> {
        // Validate connections form a single chain
        self.validate_patch(&patch)?;

        // Build the signal chain in order
        let chain = self.build_chain(&patch)?;

        Ok(PatchRuntime { patch, chain })
    }

    fn validate_patch(&self, patch: &Patch) -> Result<(), Box<dyn std::error::Error>> {
        // Build adjacency map
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for conn in &patch.connections {
            graph
                .entry(conn.from.clone())
                .or_insert_with(Vec::new)
                .push(conn.to.clone());
        }

        // Find the source (module with no incoming edges)
        let mut incoming: HashMap<String, usize> = HashMap::new();
        for module in &patch.modules {
            incoming.insert(module.id.clone(), 0);
        }
        for conn in &patch.connections {
            *incoming.entry(conn.to.clone()).or_insert(0) += 1;
        }

        let sources: Vec<_> = incoming
            .iter()
            .filter(|(_, &count)| count == 0)
            .map(|(id, _)| id.clone())
            .collect();

        if sources.is_empty() {
            return Err("No source module found (all modules have inputs)".into());
        }
        if sources.len() > 1 {
            return Err(format!("Multiple source modules found: {:?}", sources).into());
        }

        Ok(())
    }

    fn build_chain(&self, patch: &Patch) -> Result<SignalChain, Box<dyn std::error::Error>> {
        // Create module lookup
        let modules: HashMap<String, &ModuleSpec> =
            patch.modules.iter().map(|m| (m.id.clone(), m)).collect();

        // Build connection graph
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for conn in &patch.connections {
            graph
                .entry(conn.from.clone())
                .or_insert_with(Vec::new)
                .push(conn.to.clone());
        }

        // Find source module
        let mut incoming: HashMap<String, usize> = HashMap::new();
        for module in &patch.modules {
            incoming.insert(module.id.clone(), 0);
        }
        for conn in &patch.connections {
            *incoming.entry(conn.to.clone()).or_insert(0) += 1;
        }

        let source_id = incoming
            .iter()
            .find(|(_, &count)| count == 0)
            .map(|(id, _)| id.clone())
            .ok_or("No source module found")?;

        // Build chain by following connections
        let mut chain_order = Vec::new();
        let mut current = source_id;
        chain_order.push(current.clone());

        while let Some(next_nodes) = graph.get(&current) {
            if next_nodes.len() > 1 {
                return Err(format!("Module {} has multiple outputs", current).into());
            }
            if next_nodes.is_empty() {
                break;
            }
            current = next_nodes[0].clone();
            chain_order.push(current.clone());
        }

        // Build the actual modules in order
        self.build_modules_chain(patch, &chain_order, &modules)
    }

    fn build_modules_chain(
        &self,
        patch: &Patch,
        chain_order: &[String],
        modules: &HashMap<String, &ModuleSpec>,
    ) -> Result<SignalChain, Box<dyn std::error::Error>> {
        if chain_order.len() < 2 {
            return Err("Chain must have at least 2 modules".into());
        }

        // We'll build a typed chain based on the module types
        // For now, we support: clock -> melody -> voice -> dac

        // Parse the chain types
        let types: Vec<&str> = chain_order
            .iter()
            .map(|id| modules.get(id).map(|m| m.module_type.as_str()))
            .collect::<Option<Vec<_>>>()
            .ok_or("Module not found in chain")?;

        // Build based on the pattern
        match types.as_slice() {
            ["clock", "melody", "voice", "dac"] => {
                self.build_clock_melody_voice_dac(patch, chain_order, modules)
            }
            _ => Err(format!(
                "Unsupported module chain pattern: {:?}. Currently supported: [clock, melody, voice, dac]",
                types
            )
            .into()),
        }
    }

    fn build_clock_melody_voice_dac(
        &self,
        _patch: &Patch,
        chain_order: &[String],
        modules: &HashMap<String, &ModuleSpec>,
    ) -> Result<SignalChain, Box<dyn std::error::Error>> {
        // Get module specs
        let clock_spec = modules.get(&chain_order[0]).unwrap();
        let melody_spec = modules.get(&chain_order[1]).unwrap();
        let voice_spec = modules.get(&chain_order[2]).unwrap();
        let _dac_spec = modules.get(&chain_order[3]).unwrap();

        // Build Clock
        let tempo = self.build_tempo(&clock_spec.config)?;
        let mut clock = Clock::new(self.sample_rate, tempo.clone());
        if let Some(ts) = &clock_spec.config.time_signature {
            clock = clock.with_time_signature(ts.beats_per_measure);
        }

        // Build MelodyGenerator
        let (scale, melody_params) = self.build_melody_config(&melody_spec.config, &tempo)?;
        let melody = MelodyGenerator::new(
            scale,
            melody_params.clone(),
            self.sample_rate,
            tempo.clone(),
        );

        // Build Voice
        let osc_type = self.parse_oscillator_type(&voice_spec.config)?;
        let voice = Voice::new(self.sample_rate, osc_type)
            .with_osc_type_control(melody_params.oscillator_type.clone());

        // Connect the chain
        let audio_gen = clock.connect(melody).connect(voice);

        Ok(SignalChain::ClockMelodyVoiceDac {
            audio_gen,
            tempo,
            melody_params,
        })
    }

    fn build_tempo(&self, config: &ModuleConfig) -> Result<Tempo, Box<dyn std::error::Error>> {
        let bpm = config.bpm.unwrap_or(120.0);
        Ok(Tempo::new(bpm))
    }

    fn build_melody_config(
        &self,
        config: &ModuleConfig,
        _tempo: &Tempo,
    ) -> Result<(Scale, MelodyParams), Box<dyn std::error::Error>> {
        let root_note = Note::new(config.root_note.unwrap_or(60));

        let mode = if let Some(mode_str) = &config.mode {
            self.parse_mode(mode_str)?
        } else {
            Mode::Dorian
        };

        let scale = Scale::new(root_note, mode);

        let degrees = config
            .scale_degrees
            .clone()
            .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6]);

        let params = MelodyParams::new(degrees);

        if let Some(weights) = &config.note_weights {
            params.set_note_weights(weights.clone());
        }

        if let Some(duration) = config.note_duration {
            params.set_note_duration(duration);
        }

        if let Some(osc_str) = &config.oscillator_type {
            let osc_type = self.parse_oscillator_type_str(osc_str)?;
            params.set_oscillator_type(osc_type);
        }

        Ok((scale, params))
    }

    fn parse_mode(&self, mode_str: &str) -> Result<Mode, Box<dyn std::error::Error>> {
        match mode_str.to_lowercase().as_str() {
            "ionian" | "major" => Ok(Mode::Ionian),
            "dorian" => Ok(Mode::Dorian),
            "phrygian" => Ok(Mode::Phrygian),
            "lydian" => Ok(Mode::Lydian),
            "mixolydian" => Ok(Mode::Mixolydian),
            "aeolian" | "minor" => Ok(Mode::Aeolian),
            "locrian" => Ok(Mode::Locrian),
            _ => Err(format!("Unknown mode: {}", mode_str).into()),
        }
    }

    fn parse_oscillator_type(
        &self,
        config: &ModuleConfig,
    ) -> Result<OscillatorType, Box<dyn std::error::Error>> {
        let osc_str = config.oscillator_type.as_deref().unwrap_or("sine");
        self.parse_oscillator_type_str(osc_str)
    }

    fn parse_oscillator_type_str(
        &self,
        osc_str: &str,
    ) -> Result<OscillatorType, Box<dyn std::error::Error>> {
        match osc_str.to_lowercase().as_str() {
            "sine" => Ok(OscillatorType::Sine),
            "square" => Ok(OscillatorType::Square),
            "sawtooth" | "saw" => Ok(OscillatorType::Sawtooth),
            "triangle" | "tri" => Ok(OscillatorType::Triangle),
            _ => Err(format!("Unknown oscillator type: {}", osc_str).into()),
        }
    }
}

/// Runtime representation of a running patch
pub struct PatchRuntime {
    patch: Patch,
    chain: SignalChain,
}

impl PatchRuntime {
    /// Start the audio output
    pub fn start(self) -> Result<RunningPatch, Box<dyn std::error::Error>> {
        let mut dac = Dac::new()?;

        let (tempo, melody_params) = match self.chain {
            SignalChain::ClockMelodyVoiceDac {
                audio_gen,
                tempo,
                melody_params,
            } => {
                dac.start(audio_gen)?;
                (tempo, melody_params)
            }
        };

        Ok(RunningPatch {
            patch: self.patch,
            dac,
            tempo,
            melody_params,
        })
    }

    pub fn patch(&self) -> &Patch {
        &self.patch
    }
}

/// A running patch with audio output
pub struct RunningPatch {
    patch: Patch,
    dac: Dac,
    tempo: Tempo,
    melody_params: MelodyParams,
}

impl RunningPatch {
    pub fn stop(mut self) {
        self.dac.stop();
    }

    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    pub fn tempo(&self) -> &Tempo {
        &self.tempo
    }

    pub fn melody_params(&self) -> &MelodyParams {
        &self.melody_params
    }
}

/// Internal representation of the signal chain
enum SignalChain {
    ClockMelodyVoiceDac {
        audio_gen: ConnectedProcessor<
            ConnectedProcessor<Clock, MelodyGenerator, crate::ClockSignal, crate::NoteSignal>,
            Voice,
            crate::NoteSignal,
            Audio,
        >,
        tempo: Tempo,
        melody_params: MelodyParams,
    },
}
