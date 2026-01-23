//! Patch builder for constructing runnable audio graphs from patch documents.

use crate::module::{Generator, Module, Processor};
use crate::patch::{ModuleConfig, ModuleSpec, Patch};
use crate::scale::{Mode, Note, Scale};
use crate::sequencer::MelodyParams;
use crate::time::{Clock, Tempo};
use crate::{Audio, Dac, MelodyGenerator, OscillatorType, Voice};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// Constructs a runnable audio graph from a [`Patch`] document.
///
/// Validates the patch structure, builds the signal routing graph,
/// and produces a [`PatchRuntime`] that can be started for playback.
pub struct PatchBuilder {
    sample_rate: u32,
}

impl PatchBuilder {
    /// Creates a new patch builder with the given sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Builds a runnable patch from a patch document.
    ///
    /// Validates the patch structure and constructs the signal graph.
    /// Returns a [`PatchRuntime`] that can be started for audio playback.
    pub fn build_and_run(&self, patch: Patch) -> Result<PatchRuntime, Box<dyn std::error::Error>> {
        // Validate the patch
        self.validate_patch(&patch)?;

        // Build the signal graph
        let chain = self.build_graph(&patch)?;

        Ok(PatchRuntime { patch, chain })
    }

    /// Validates the patch structure for correctness.
    ///
    /// Checks that all connections reference existing modules, the graph
    /// is acyclic, there is exactly one source (clock) and one sink (DAC).
    fn validate_patch(&self, patch: &Patch) -> Result<(), Box<dyn std::error::Error>> {
        // Check that all connections reference existing modules
        let module_ids: HashSet<String> = patch.modules.iter().map(|m| m.id.clone()).collect();

        for conn in &patch.connections {
            if !module_ids.contains(&conn.from) {
                return Err(format!("Connection references unknown module: {}", conn.from).into());
            }
            if !module_ids.contains(&conn.to) {
                return Err(format!("Connection references unknown module: {}", conn.to).into());
            }
        }

        // Check for cycles using DFS
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for conn in &patch.connections {
            graph
                .entry(conn.from.clone())
                .or_insert_with(Vec::new)
                .push(conn.to.clone());
        }

        for module_id in &module_ids {
            let mut visited = HashSet::new();
            let mut stack = HashSet::new();
            if self.has_cycle(&graph, module_id, &mut visited, &mut stack) {
                return Err(format!("Cycle detected involving module: {}", module_id).into());
            }
        }

        // Find source modules (no incoming edges)
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

        // Find sink modules (no outgoing edges)
        let sinks: Vec<_> = module_ids
            .iter()
            .filter(|id| !graph.contains_key(*id) || graph[*id].is_empty())
            .cloned()
            .collect();

        if sinks.len() != 1 {
            return Err(
                format!("Expected exactly one sink module (DAC), found: {:?}", sinks).into(),
            );
        }

        // Check that the sink is a DAC
        let sink_module = patch.modules.iter().find(|m| m.id == sinks[0]).unwrap();
        if sink_module.module_type != "dac" {
            return Err(format!(
                "Sink module must be of type 'dac', found: {}",
                sink_module.module_type
            )
            .into());
        }

        Ok(())
    }

    /// Detects cycles in the signal graph using depth-first search.
    fn has_cycle(
        &self,
        graph: &HashMap<String, Vec<String>>,
        node: &str,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
    ) -> bool {
        if stack.contains(node) {
            return true;
        }
        if visited.contains(node) {
            return false;
        }

        visited.insert(node.to_string());
        stack.insert(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if self.has_cycle(graph, neighbor, visited, stack) {
                    return true;
                }
            }
        }

        stack.remove(node);
        false
    }

    /// Constructs the internal signal graph from the validated patch.
    fn build_graph(&self, patch: &Patch) -> Result<SignalGraph, Box<dyn std::error::Error>> {
        // Build connection maps
        let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
        let mut incoming: HashMap<String, Vec<String>> = HashMap::new();

        for conn in &patch.connections {
            outgoing
                .entry(conn.from.clone())
                .or_insert_with(Vec::new)
                .push(conn.to.clone());
            incoming
                .entry(conn.to.clone())
                .or_insert_with(Vec::new)
                .push(conn.from.clone());
        }

        // Find source (clock) and create it
        let module_map: HashMap<String, &ModuleSpec> =
            patch.modules.iter().map(|m| (m.id.clone(), m)).collect();

        let source_id = patch
            .modules
            .iter()
            .find(|m| !incoming.contains_key(&m.id))
            .ok_or("No source module found")?
            .id
            .clone();

        let source_spec = module_map[&source_id];
        if source_spec.module_type != "clock" {
            return Err(format!(
                "Source module must be 'clock', found: {}",
                source_spec.module_type
            )
            .into());
        }

        let tempo = self.build_tempo(&source_spec.config)?;
        let clock = Arc::new(Mutex::new(
            Clock::new(self.sample_rate, tempo.clone()).with_time_signature(
                source_spec
                    .config
                    .time_signature
                    .as_ref()
                    .map(|ts| ts.beats_per_measure)
                    .unwrap_or(4),
            ),
        ));

        // Build parallel paths from clock to DAC
        let dac_id = patch
            .modules
            .iter()
            .find(|m| m.module_type == "dac")
            .ok_or("No DAC module found")?
            .id
            .clone();

        // Find all paths from clock to DAC inputs
        let dac_inputs = incoming.get(&dac_id).cloned().unwrap_or_default();

        let mut voices = Vec::new();
        let mut melody_params_list = Vec::new();

        for input_id in &dac_inputs {
            // Build the chain from clock to this DAC input
            let (voice, melody_params) = self.build_voice_chain(
                patch,
                &source_id,
                input_id,
                &module_map,
                &incoming,
                &outgoing,
                clock.clone(),
                &tempo,
            )?;

            voices.push(voice);
            if let Some(params) = melody_params {
                melody_params_list.push(params);
            }
        }

        Ok(SignalGraph {
            clock,
            voices,
            tempo,
            melody_params_list,
        })
    }

    /// Builds a voice chain from a clock source to a DAC input.
    ///
    /// Traces the path from source to target and constructs the
    /// melody generator and voice modules along the way.
    fn build_voice_chain(
        &self,
        _patch: &Patch,
        source_id: &str,
        target_id: &str,
        module_map: &HashMap<String, &ModuleSpec>,
        _incoming: &HashMap<String, Vec<String>>,
        outgoing: &HashMap<String, Vec<String>>,
        _clock: Arc<Mutex<Clock>>,
        tempo: &Tempo,
    ) -> Result<(VoiceChain, Option<MelodyParams>), Box<dyn std::error::Error>> {
        // Trace path from source to target
        let path = self.find_path(source_id, target_id, outgoing)?;

        if path.len() < 3 {
            return Err(format!("Path too short: {:?}", path).into());
        }

        // Expected pattern: clock -> melody -> voice
        if path.len() != 3 {
            return Err(format!(
                "Expected path length 3 (clock->melody->voice), got {}: {:?}",
                path.len(),
                path
            )
            .into());
        }

        let melody_spec = module_map[&path[1]];
        let voice_spec = module_map[&path[2]];

        if melody_spec.module_type != "melody" {
            return Err(
                format!("Expected melody module, found: {}", melody_spec.module_type).into(),
            );
        }
        if voice_spec.module_type != "voice" {
            return Err(format!("Expected voice module, found: {}", voice_spec.module_type).into());
        }

        // Build melody generator
        let (scale, melody_params) = self.build_melody_config(&melody_spec.config, tempo)?;
        let melody = MelodyGenerator::new(
            scale,
            melody_params.clone(),
            self.sample_rate,
            tempo.clone(),
        );

        // Build voice
        let osc_type = self.parse_oscillator_type(&voice_spec.config)?;
        let voice = Voice::new(self.sample_rate, osc_type)
            .with_osc_type_control(melody_params.oscillator_type.clone());

        Ok((
            VoiceChain {
                melody: Arc::new(Mutex::new(melody)),
                voice: Arc::new(Mutex::new(voice)),
            },
            Some(melody_params),
        ))
    }

    /// Finds the shortest path between two modules using breadth-first search.
    fn find_path(
        &self,
        start: &str,
        end: &str,
        graph: &HashMap<String, Vec<String>>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        let mut parent: HashMap<String, String> = HashMap::new();

        queue.push_back(start.to_string());
        visited.insert(start.to_string());

        while let Some(node) = queue.pop_front() {
            if node == end {
                // Reconstruct path
                let mut path = Vec::new();
                let mut current = end.to_string();
                path.push(current.clone());

                while let Some(p) = parent.get(&current) {
                    path.push(p.clone());
                    current = p.clone();
                }

                path.reverse();
                return Ok(path);
            }

            if let Some(neighbors) = graph.get(&node) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        parent.insert(neighbor.clone(), node.clone());
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        Err(format!("No path from {} to {}", start, end).into())
    }

    /// Creates a Tempo from the clock module's configuration.
    fn build_tempo(&self, config: &ModuleConfig) -> Result<Tempo, Box<dyn std::error::Error>> {
        let bpm = config.bpm.unwrap_or(120.0);
        Ok(Tempo::new(bpm))
    }

    /// Creates a Scale and MelodyParams from a melody module's configuration.
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

    /// Parses a mode string into a Mode enum value.
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

    /// Extracts and parses the oscillator type from a module configuration.
    fn parse_oscillator_type(
        &self,
        config: &ModuleConfig,
    ) -> Result<OscillatorType, Box<dyn std::error::Error>> {
        let osc_str = config.oscillator_type.as_deref().unwrap_or("sine");
        self.parse_oscillator_type_str(osc_str)
    }

    /// Parses an oscillator type string into an OscillatorType enum value.
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

/// A single voice processing chain (melody generator → voice).
///
/// Processes clock input through melody generation and voice synthesis
/// to produce audio output.
struct VoiceChain {
    melody: Arc<Mutex<MelodyGenerator>>,
    voice: Arc<Mutex<Voice>>,
}

impl VoiceChain {
    /// Processes one sample through the voice chain.
    fn process_and_output(&self, clock_signal: crate::ClockSignal) -> Audio {
        let mut melody = self.melody.lock().unwrap();
        let mut voice = self.voice.lock().unwrap();

        melody.process();
        voice.process();

        let note_signal = melody.process_signal(clock_signal);
        Audio::new(voice.process_signal(note_signal).value)
    }
}

/// Internal representation of the complete signal processing graph.
///
/// Contains the master clock and all voice chains. Processes the entire
/// graph each sample and mixes the voices together.
struct SignalGraph {
    clock: Arc<Mutex<Clock>>,
    voices: Vec<VoiceChain>,
    tempo: Tempo,
    melody_params_list: Vec<MelodyParams>,
}

impl SignalGraph {
    /// Processes one sample through the entire graph and mixes all voices.
    fn process_and_output(&self) -> Audio {
        let mut clock = self.clock.lock().unwrap();
        clock.process();
        let clock_signal = clock.output();
        drop(clock); // Release lock before processing voices

        // Process all voice chains
        let mut samples = Vec::new();
        for voice_chain in &self.voices {
            let sample = voice_chain.process_and_output(clock_signal);
            samples.push(sample);
        }

        // Mix them together
        if samples.is_empty() {
            Audio::silence()
        } else if samples.len() == 1 {
            samples[0]
        } else {
            let gain = 1.0 / (samples.len() as f32).sqrt();
            let mixed: f32 = samples.iter().map(|s| s.value).sum();
            Audio::new(mixed * gain)
        }
    }
}

/// A validated patch ready to be started.
///
/// Created by [`PatchBuilder::build_and_run`]. Call [`start`](Self::start)
/// to begin audio playback.
pub struct PatchRuntime {
    patch: Patch,
    chain: SignalGraph,
}

impl PatchRuntime {
    /// Starts audio playback and returns a handle to the running patch.
    pub fn start(self) -> Result<RunningPatch, Box<dyn std::error::Error>> {
        let tempo = self.chain.tempo.clone();
        let melody_params_list = self.chain.melody_params_list.clone();

        // Create a generator that wraps the signal graph
        let graph = Arc::new(Mutex::new(self.chain));
        let audio_gen = GraphGenerator { graph };

        let mut dac = Dac::new()?;
        dac.start(audio_gen)?;

        Ok(RunningPatch {
            patch: self.patch,
            dac,
            tempo,
            melody_params_list,
        })
    }

    pub fn patch(&self) -> &Patch {
        &self.patch
    }
}

/// Adapter that implements [`Generator`] for a [`SignalGraph`].
///
/// Allows the signal graph to be used as an audio source for the DAC.
struct GraphGenerator {
    graph: Arc<Mutex<SignalGraph>>,
}

impl Module for GraphGenerator {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "GraphGenerator"
    }
}

impl Generator<Audio> for GraphGenerator {
    fn output(&mut self) -> Audio {
        let graph = self.graph.lock().unwrap();
        graph.process_and_output()
    }
}

/// A running patch with active audio output.
///
/// Holds the DAC stream and provides access to runtime parameters
/// like tempo and melody settings. Call [`stop`](Self::stop) to
/// end playback.
pub struct RunningPatch {
    patch: Patch,
    dac: Dac,
    tempo: Tempo,
    melody_params_list: Vec<MelodyParams>,
}

impl RunningPatch {
    /// Stops audio playback and releases the audio device.
    pub fn stop(mut self) {
        self.dac.stop();
    }

    /// Returns a reference to the patch document.
    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Returns a reference to the tempo controller for BPM adjustments.
    pub fn tempo(&self) -> &Tempo {
        &self.tempo
    }

    /// Returns the melody parameters for the first voice.
    ///
    /// Use [`all_melody_params`](Self::all_melody_params) for multi-voice patches.
    pub fn melody_params(&self) -> &MelodyParams {
        &self.melody_params_list[0]
    }

    /// Returns melody parameters for all voices in the patch.
    pub fn all_melody_params(&self) -> &[MelodyParams] {
        &self.melody_params_list
    }
}
