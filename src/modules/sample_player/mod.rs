//! Sample player module for audio file playback.
//!
//! Two playback modes, chosen by the `mode` config key:
//! - `"classic"` (default): the fractional read head advances by
//!   `pitch_ratio`, so speed and pitch stay coupled (tape-style).
//! - `"elastic"`: a shared WSOLA reader decouples them; `time_ratio`
//!   controls speed and `pitch_ratio` controls pitch independently.

use std::sync::Arc;

use crate::control_request::{
    local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable, Refusal,
    RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::modules::sample_loading::elastic::ElasticReader;
use crate::modules::sample_loading::{elastic_mode_from_config, AssetLoader, SampleData};
use crate::payload::{Payload, Retired, Shared};
use crate::traits::ControlMeta;
use crate::Module;

use self::table::{ASSET, LOOP, MIN_RATIO, PITCH_RATIO, PLAY, PLAYING, TIME_RATIO};

mod inputs;
mod outputs;
mod table;

pub struct SamplePlayerFactory;

impl ModuleFactory for SamplePlayerFactory {
    fn type_id(&self) -> &'static str {
        "sample_player"
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        use crate::module_config::ConfigKey;
        const {
            &[
                ConfigKey::json("asset"),
                ConfigKey::boolean("autoplay"),
                ConfigKey::boolean("loop"),
                ConfigKey::text("mode"),
            ]
        }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let (player, surface) = build(sample_rate, config)?;
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(player)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A player built from `config`, and its surface.
fn build(
    sample_rate: u32,
    config: &serde_json::Value,
) -> Result<(SamplePlayer, DeclaredSurface), Box<dyn std::error::Error>> {
    let asset = asset_from_config(config)?;
    let elastic = elastic_mode_from_config("sample_player", config)?;
    let loader = AssetLoader::current(sample_rate);
    let table = table::table(elastic);
    let cells = Arc::new(ControlCells::new(table::defaults(elastic)));
    let surface = DeclaredSurface::new(table.clone(), cells.clone());
    let sample = match asset.as_deref().filter(|asset| !asset.is_empty()) {
        Some(asset) => {
            let sample = loader.load(asset)?;
            if elastic {
                // Computed here, off the audio thread, which finds it
                // on the sample through a lock-free accessor.
                sample.elastic_analysis();
            }
            Shared::from_arc(sample)
        }
        None => Shared::new(SampleData::empty(sample_rate)),
    };
    surface.show_payload("asset", asset.unwrap_or_default().into());
    // `autoplay` is the authored start state. `play`, the control's
    // own key, is read as the same: a document saved while `play` was a
    // state recorded it there. Where both are given, `play` wins.
    let start = start_from_config(config, "autoplay")?;
    let start = start_from_config(config, "play")?.or(start);
    crate::factory::apply_control_keys(&surface, config, |key| {
        matches!(key, "loop" | "pitch_ratio" | "time_ratio")
    })?;
    let player = SamplePlayer::build(sample_rate, table, cells, sample, start.unwrap_or(false));
    Ok((player, surface.with_context(Arc::new(loader))))
}

/// A start state in config: a boolean, or one spelled as text.
fn start_from_config(config: &serde_json::Value, key: &str) -> Result<Option<bool>, String> {
    match config.get(key) {
        None => Ok(None),
        Some(serde_json::Value::Bool(flag)) => Ok(Some(*flag)),
        Some(serde_json::Value::String(text)) if text.trim() == "true" => Ok(Some(true)),
        Some(serde_json::Value::String(text)) if text.trim() == "false" => Ok(Some(false)),
        Some(other) => Err(format!("config '{key}' must be a boolean, got {other}")),
    }
}

/// Reads the shared `asset` config key used by sample-backed modules.
/// Invention loads resolve `asset` before factories run; live module
/// additions may still supply a package ref here.
pub(crate) fn asset_from_config(config: &serde_json::Value) -> Result<Option<String>, String> {
    config
        .get("asset")
        .map(|value| {
            serde_json::from_value::<crate::pkg::AudioAssetRef>(value.clone())
                .map_err(|err| format!("invalid asset reference {}: {}", value, err))
        })
        .transpose()
        .map(|asset| {
            asset.map(|asset| match asset {
                crate::pkg::AudioAssetRef::Text(text) => text,
                crate::pkg::AudioAssetRef::Local { path } => path,
            })
        })
}

/// Plays one sample, from its start, on a `play` request or a rising
/// `play` gate; once through, or looping.
///
/// Its controls are applied on the thread running it, between blocks, as
/// requests (see `crate::control_request`): `play` is an event, so every
/// start restarts the sample, and `play = false` stops it. Only requests
/// write it; the player reports whether it is playing in `playing`.
pub struct SamplePlayer {
    table: &'static ControlTable,
    cells: Arc<ControlCells>,
    inputs: inputs::SamplePlayerInputs,
    outputs: outputs::SamplePlayerOutputs,
    sample: Shared<SampleData>,
    position: f64,
    /// Present iff the module was built in elastic mode.
    reader: Option<ElasticReader>,
    playing: bool,
    /// Whether playback is asked for: on with a start, off with a stop or
    /// as a one-shot ends. A sample swapped in starts playing when it is.
    armed: bool,
    loop_enabled: bool,
    pitch_ratio: f32,
    time_ratio: f32,
    last_play_input: f32,
    pending_start_gate: bool,
    /// What `playing` was last published as.
    shown_playing: bool,
}

impl SamplePlayer {
    /// A player holding `sample`, its controls as `cells` hold them, playing
    /// from the start when `start` is set.
    fn build(
        sample_rate: u32,
        table: &'static ControlTable,
        cells: Arc<ControlCells>,
        sample: Shared<SampleData>,
        start: bool,
    ) -> Self {
        let number = |index| match cells.load(index) {
            Some(RtValue::F32(value)) => value.max(MIN_RATIO),
            _ => 1.0,
        };
        let elastic = table.decl(TIME_RATIO).is_some();
        let mut player = Self {
            table,
            inputs: inputs::SamplePlayerInputs::new(),
            outputs: outputs::SamplePlayerOutputs::new(),
            sample,
            position: 0.0,
            reader: elastic.then(|| ElasticReader::new(sample_rate)),
            playing: false,
            armed: start,
            loop_enabled: matches!(cells.load(LOOP), Some(RtValue::Bool(true))),
            pitch_ratio: number(PITCH_RATIO),
            time_ratio: number(TIME_RATIO),
            last_play_input: 0.0,
            pending_start_gate: false,
            shown_playing: false,
            cells,
        };
        if start {
            // As a start written before the first block: the sample swapped
            // in plays, and the start restarts it.
            player.playing = player.sample.len() > 0;
            player.restart();
        }
        player.cells.publish(PLAY, RtValue::Bool(start));
        player.publish_playing();
        player
    }

    fn restart(&mut self) {
        let was_audible = self.playing;
        self.position = 0.0;
        self.playing = self.sample.len() > 0;
        self.pending_start_gate = self.playing;
        if let Some(reader) = self.reader.as_mut() {
            if was_audible {
                reader.reset_crossfade(0.0);
            } else {
                reader.reset(0.0);
            }
        }
    }

    fn stop(&mut self) {
        self.playing = false;
        self.position = 0.0;
        self.pending_start_gate = false;
        if let Some(reader) = self.reader.as_mut() {
            reader.reset(0.0);
        }
    }

    /// Publishes whether it is playing. Wait-free.
    fn publish_playing(&mut self) {
        self.shown_playing = self.playing;
        self.cells.publish(PLAYING, RtValue::Bool(self.playing));
    }
}

impl Module for SamplePlayer {
    fn name(&self) -> &str {
        "SamplePlayer"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            if self.inputs.play(i) > 0.5 && self.last_play_input <= 0.5 {
                self.restart();
            }

            let loop_enabled = self.inputs.loop_enabled(i, self.loop_enabled);
            let mut start_gate = 0.0;
            let mut end_gate = 0.0;
            let mut left = 0.0;
            let mut right = 0.0;

            if self.pending_start_gate && self.playing {
                start_gate = 1.0;
                self.pending_start_gate = false;
            }

            let sample = &*self.sample;
            let len = sample.len();
            if self.playing && len > 0 {
                let pitch = self.inputs.pitch_ratio(i, self.pitch_ratio).max(MIN_RATIO);

                if let Some(reader) = self.reader.as_mut() {
                    // Elastic: the reader decouples time (nominal head
                    // advance) from pitch (in-window read step).
                    if let Some(analysis) = sample.cached_elastic_analysis() {
                        let time = self.time_ratio;
                        if let Some((l, r)) =
                            reader.next(sample, analysis, 0.0, len as f64, time, pitch)
                        {
                            left = l;
                            right = r;
                        }
                    }

                    if reader.source_position() >= len as f64 {
                        end_gate = 1.0;
                        if loop_enabled {
                            reader.reset_crossfade(0.0);
                            self.pending_start_gate = true;
                        } else {
                            self.playing = false;
                            self.armed = false;
                            reader.reset(0.0);
                        }
                    }
                } else {
                    let (l, r) = sample.sample_at(self.position);
                    left = l;
                    right = r;

                    self.position += pitch as f64;

                    if self.position >= len as f64 {
                        end_gate = 1.0;
                        if loop_enabled {
                            // `%=` keeps the fractional read head bounded without
                            // accumulating drift across loops.
                            self.position %= len as f64;
                            self.pending_start_gate = true;
                        } else {
                            self.playing = false;
                            self.armed = false;
                            self.position = 0.0;
                        }
                    }
                }
            }

            self.outputs.set(i, left, right, start_gate, end_gate);
            self.last_play_input = self.inputs.play(i);
        }
        if self.playing != self.shown_playing {
            self.publish_playing();
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((self.table, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (PLAY, RtValue::Bool(true)) => {
                self.armed = true;
                self.restart();
                self.publish_playing();
            }
            (PLAY, RtValue::Bool(false)) => {
                self.armed = false;
                self.stop();
                self.publish_playing();
            }
            (LOOP, RtValue::Bool(flag)) => self.loop_enabled = flag,
            (PITCH_RATIO, RtValue::F32(ratio)) => {
                self.pitch_ratio = ratio.max(MIN_RATIO);
                return Ok(RtValue::F32(self.pitch_ratio));
            }
            (TIME_RATIO, RtValue::F32(ratio)) if self.reader.is_some() => {
                self.time_ratio = ratio.max(MIN_RATIO);
                return Ok(RtValue::F32(self.time_ratio));
            }
            _ => return Err(Refusal::Unsupported),
        }
        Ok(value)
    }

    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        if control != ASSET {
            return Err((Refusal::Unsupported, payload));
        }
        let sample = match payload.downcast::<SampleData>() {
            Ok(sample) => sample,
            Err(payload) => return Err((Refusal::Invalid, payload)),
        };
        let replaced = std::mem::replace(&mut self.sample, sample);
        // The new sample plays from its start when playback is asked for.
        self.position = 0.0;
        self.playing = self.armed && self.sample.len() > 0;
        self.pending_start_gate = self.playing;
        if let Some(reader) = self.reader.as_mut() {
            reader.reset(0.0);
        }
        self.publish_playing();
        Ok(replaced.into())
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}

#[cfg(test)]
mod tests;
