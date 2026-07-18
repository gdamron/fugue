//! Indexed slice playback for loops, breakbeats, and stem packs.
//!
//! `sample_slicer` loads one audio file and a set of frame-addressed slices.
//! A rising edge on `trigger` latches the zero-based `slice` input and plays
//! that region through its exclusive end. Triggering again restarts playback
//! immediately at the newly selected slice.
//!
//! Slice points may be supplied directly in module config:
//!
//! ```json
//! {
//!   "asset": { "path": "./break.wav" },
//!   "slices": [
//!     { "start_frames": 0, "end_frames": 22050, "name": "kick" },
//!     { "start_frames": 22050, "end_frames": 44100, "name": "snare" }
//!   ]
//! }
//! ```
//!
//! When `slices` is omitted and the asset belongs to an installed
//! `sample-pack`, the matching file entry in that pack's manifest supplies
//! them. Frame addresses are always interpreted in the source file's sample
//! rate and scaled once at load time to the engine rate.
//!
//! With `mode: "elastic"` in config, slices play through the shared WSOLA
//! reader and the module gains a control surface with independent
//! `time_ratio` (speed) and `pitch_ratio` (pitch) controls, so slices can
//! be tempo-fit without chipmunking. Under a `time_ratio` a slice occupies
//! a scaled number of output frames, and `slice_end_gate` still fires when
//! the end of the *source* region is reached.
//!
//! # Inputs
//! - `trigger`: Rising edges start or retrigger the selected slice.
//! - `slice`: Zero-based slice index, rounded to the nearest integer.
//!
//! # Outputs
//! - `audio_left`, `audio_right`: Stereo sample playback.
//! - `slice_start_gate`: One-sample pulse when valid slice playback starts.
//! - `slice_end_gate`: One-sample pulse on the last frame of a slice.

use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::pkg::SampleSlice;
use crate::Module;

use super::sample_loading::elastic::ElasticReader;
use super::sample_loading::{elastic_mode_from_config, load_sample_source, SampleData};
use super::sample_player::source_from_config;

pub use self::controls::SampleSlicerControls;

mod controls;
mod inputs;
mod outputs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SliceRange {
    start: usize,
    end: usize,
}

/// Factory for the built-in `sample_slicer` module.
pub struct SampleSlicerFactory;

impl ModuleFactory for SampleSlicerFactory {
    fn type_id(&self) -> &'static str {
        "sample_slicer"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let source = source_from_config(config)?
            .ok_or("sample_slicer config requires 'asset' or 'source'")?;
        let (resolved_source, sample) = load_sample_source(&source, sample_rate)?;

        let slices = match config.get("slices") {
            Some(value) => serde_json::from_value::<Vec<SampleSlice>>(value.clone())
                .map_err(|err| format!("sample_slicer: invalid slices: {}", err))?,
            None => slices_from_sample_pack(&resolved_source)?,
        };
        let ranges = resolve_ranges(&slices, &sample)?;
        let initial_slice = config
            .get("slice")
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    "sample_slicer: 'slice' must be a non-negative integer".to_string()
                })
            })
            .transpose()?
            .unwrap_or(0);
        let initial_slice =
            usize::try_from(initial_slice).map_err(|_| "sample_slicer: 'slice' is too large")?;
        if initial_slice >= ranges.len() {
            return Err(format!(
                "sample_slicer: initial slice {} is out of range for {} slices",
                initial_slice,
                ranges.len()
            )
            .into());
        }

        let elastic = elastic_mode_from_config("sample_slicer", config)?;
        let (voice, handles, control_surface) = if elastic {
            // Analysis is computed here, off the audio thread, and cached on
            // the (process-cached) buffer for every module sharing it.
            sample.elastic_analysis();
            let controls = SampleSlicerControls::new();
            let voice = ElasticVoice {
                reader: ElasticReader::new(sample_rate),
                controls: controls.clone(),
                start: 0.0,
                end: 0.0,
            };
            let handles: Vec<(String, Arc<dyn std::any::Any + Send + Sync>)> = vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )];
            (
                Some(voice),
                handles,
                Some(Arc::new(controls) as Arc<dyn crate::ControlSurface + Send + Sync>),
            )
        } else {
            (None, Vec::new(), None)
        };

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(SampleSlicer::new(
                sample,
                ranges,
                initial_slice,
                voice,
            ))),
            handles,
            control_surface,
            sink: None,
        })
    }

    fn input_ports(&self) -> Option<&'static [&'static str]> {
        Some(&inputs::INPUTS)
    }

    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&outputs::OUTPUTS)
    }
}

/// One elastic playback voice: the WSOLA reader plus the live ratios and
/// the active slice region in source frames.
struct ElasticVoice {
    reader: ElasticReader,
    controls: SampleSlicerControls,
    start: f64,
    end: f64,
}

/// Allocation-free, lock-free indexed slice player.
pub struct SampleSlicer {
    sample: Arc<SampleData>,
    slices: Vec<SliceRange>,
    inputs: inputs::SampleSlicerInputs,
    outputs: outputs::SampleSlicerOutputs,
    position: usize,
    active_end: usize,
    /// Present iff the module was built in elastic mode.
    elastic: Option<ElasticVoice>,
    playing: bool,
    last_trigger: f32,
}

impl SampleSlicer {
    fn new(
        sample: Arc<SampleData>,
        slices: Vec<SliceRange>,
        initial_slice: usize,
        elastic: Option<ElasticVoice>,
    ) -> Self {
        Self {
            sample,
            slices,
            inputs: inputs::SampleSlicerInputs::new(initial_slice),
            outputs: outputs::SampleSlicerOutputs::new(),
            position: 0,
            active_end: 0,
            elastic,
            playing: false,
            last_trigger: 0.0,
        }
    }

    #[inline]
    fn trigger_slice(&mut self, index: usize) -> bool {
        let Some(range) = self.slices.get(index).copied() else {
            self.playing = false;
            return false;
        };
        if let Some(voice) = self.elastic.as_mut() {
            voice.start = range.start as f64;
            voice.end = range.end as f64;
            if self.playing {
                // Retrigger while audible: crossfade out of the old audio
                // instead of cutting.
                voice.reader.reset_crossfade(voice.start);
            } else {
                voice.reader.reset(voice.start);
            }
        }
        self.position = range.start;
        self.active_end = range.end;
        self.playing = true;
        true
    }
}

impl Module for SampleSlicer {
    fn name(&self) -> &str {
        "SampleSlicer"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let trigger = self.inputs.trigger(i);
            let rising = trigger > 0.5 && self.last_trigger <= 0.5;
            let mut start_gate = 0.0;
            let mut end_gate = 0.0;

            if rising && self.trigger_slice(self.inputs.slice(i)) {
                start_gate = 1.0;
            }

            let (left, right) = if self.playing {
                if let Some(voice) = self.elastic.as_mut() {
                    let time = voice.controls.time_ratio();
                    let pitch = voice.controls.pitch_ratio();
                    let mut frame = (0.0, 0.0);
                    if let Some(analysis) = self.sample.cached_elastic_analysis() {
                        if let Some(value) = voice.reader.next(
                            &self.sample,
                            analysis,
                            voice.start,
                            voice.end,
                            time,
                            pitch,
                        ) {
                            frame = value;
                        }
                    }
                    // The nominal head advances by time_ratio per output
                    // frame, so the gate fires when the *source* region end
                    // is reached and a slice occupies len/time_ratio frames.
                    if voice.reader.source_position() >= voice.end {
                        self.playing = false;
                        end_gate = 1.0;
                    }
                    frame
                } else {
                    let frame = self.sample.sample_at(self.position as f64);
                    self.position += 1;
                    if self.position >= self.active_end {
                        self.playing = false;
                        end_gate = 1.0;
                    }
                    frame
                }
            } else {
                (0.0, 0.0)
            };

            self.outputs.set(i, left, right, start_gate, end_gate);
            self.last_trigger = trigger;
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
}

fn resolve_ranges(slices: &[SampleSlice], sample: &SampleData) -> Result<Vec<SliceRange>, String> {
    if slices.is_empty() {
        return Err("sample_slicer: at least one slice is required".to_string());
    }

    let mut ranges = Vec::with_capacity(slices.len());
    for (index, slice) in slices.iter().enumerate() {
        if slice.end_frames <= slice.start_frames {
            return Err(format!(
                "sample_slicer: slice {} end_frames must exceed start_frames",
                index
            ));
        }
        let start = sample.scaled_frame(slice.start_frames)?;
        let end = sample.scaled_frame(slice.end_frames)?;
        if end <= start {
            return Err(format!(
                "sample_slicer: slice {} collapses after sample-rate conversion",
                index
            ));
        }
        if end > sample.len() {
            return Err(format!(
                "sample_slicer: slice {} ends at frame {}, beyond sample length {}",
                index,
                slice.end_frames,
                sample.len()
            ));
        }
        ranges.push(SliceRange { start, end });
    }
    Ok(ranges)
}

#[cfg(not(target_arch = "wasm32"))]
fn slices_from_sample_pack(source: &str) -> Result<Vec<SampleSlice>, String> {
    use std::path::{Path, PathBuf};

    if source.starts_with("http://") || source.starts_with("https://") {
        return Err("sample_slicer: remote assets require explicit 'slices'".to_string());
    }

    let authored_path = Path::new(source);
    let asset_path =
        std::fs::canonicalize(authored_path).unwrap_or_else(|_| PathBuf::from(authored_path));
    for root in asset_path.parent().into_iter().flat_map(Path::ancestors) {
        let package_path = root.join("fugue.pkg.json");
        if !package_path.is_file() {
            continue;
        }
        let package = crate::pkg::parse_path(&package_path).map_err(|err| {
            format!(
                "sample_slicer: failed to load {}: {}",
                package_path.display(),
                err
            )
        })?;
        let crate::pkg::EntrySpec::SamplePack { samples } = package.entry else {
            continue;
        };
        let relative = asset_path.strip_prefix(root).map_err(|_| {
            format!(
                "sample_slicer: asset {} is outside package {}",
                asset_path.display(),
                root.display()
            )
        })?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        let manifest_path = root.join(samples);
        let manifest = crate::pkg::parse_sample_pack_path(&manifest_path).map_err(|err| {
            format!(
                "sample_slicer: failed to load {}: {}",
                manifest_path.display(),
                err
            )
        })?;
        let file = manifest
            .files
            .iter()
            .find(|file| file.path == relative)
            .ok_or_else(|| {
                format!(
                    "sample_slicer: '{}' is not listed in {}",
                    relative,
                    manifest_path.display()
                )
            })?;
        return Ok(file.slices.clone());
    }

    Err(
        "sample_slicer: config requires 'slices' when the asset is not in a sample-pack"
            .to_string(),
    )
}

#[cfg(target_arch = "wasm32")]
fn slices_from_sample_pack(_source: &str) -> Result<Vec<SampleSlice>, String> {
    Err("sample_slicer: wasm assets require explicit 'slices'".to_string())
}

#[cfg(test)]
mod tests;
