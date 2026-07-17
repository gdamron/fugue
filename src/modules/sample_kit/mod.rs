//! Sample kit module: maps a trigger's value (or MIDI note number, or named
//! index) to one of N preloaded sample slots. The canonical drum kit.
//!
//! ```json
//! {
//!   "type": "sample_kit",
//!   "config": {
//!     "samples": [
//!       { "key": 36, "asset": "fugue.drums.808@1.2.0:kick.wav" },
//!       { "key": 38, "asset": "fugue.drums.808@1.2.0:snare.wav" },
//!       { "key": "ride", "asset": { "path": "./samples/ride.wav" }, "gain": 0.8 }
//!     ]
//!   }
//! }
//! ```
//!
//! Every slot's buffer is decoded and resampled to the engine rate at build
//! time (or in a control-thread `asset.<i>` call), so the audio thread only
//! mixes: one voice per slot, retrigger restarts the slot, distinct slots
//! overlap freely.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::modules::sample_loading::SampleData;
use crate::Module;

pub use self::controls::{SampleKitControls, SlotKey, SlotSpec};

mod controls;
mod inputs;
mod outputs;

pub struct SampleKitFactory;

impl ModuleFactory for SampleKitFactory {
    fn type_id(&self) -> &'static str {
        "sample_kit"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let specs = parse_config(config)?;
        let (controls, samples) = SampleKitControls::new(sample_rate, specs)?;
        let kit = SampleKit::new_with_controls(controls.clone(), samples);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(kit)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Parses the `samples` slot list. A missing or null config builds an empty
/// kit (module type discovery constructs every type with a null config).
fn parse_config(config: &serde_json::Value) -> Result<Vec<SlotSpec>, String> {
    let Some(samples) = config.get("samples") else {
        return Ok(Vec::new());
    };
    let Some(entries) = samples.as_array() else {
        return Err("'samples' must be an array of slot objects".to_string());
    };

    let mut specs: Vec<SlotSpec> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let slot = entry
            .as_object()
            .ok_or_else(|| format!("samples[{}] must be an object", index))?;

        let key = parse_key(slot.get("key"), index)?;
        if specs.iter().any(|spec| spec.key == key) {
            return Err(format!("samples[{}]: duplicate key '{}'", index, key));
        }

        let asset = slot
            .get("asset")
            .ok_or_else(|| format!("samples[{}] is missing 'asset'", index))?;
        let asset: crate::pkg::AudioAssetRef = serde_json::from_value(asset.clone())
            .map_err(|err| format!("samples[{}]: invalid asset reference: {}", index, err))?;
        let asset = match asset {
            crate::pkg::AudioAssetRef::Text(text) => text,
            crate::pkg::AudioAssetRef::Local { path } => path,
        };

        let gain = match slot.get("gain") {
            Some(value) => value
                .as_f64()
                .ok_or_else(|| format!("samples[{}]: 'gain' must be a number", index))?
                as f32,
            None => 1.0,
        };

        specs.push(SlotSpec { key, asset, gain });
    }
    Ok(specs)
}

/// A slot key is a JSON integer (trigger value / MIDI note number) or a
/// non-numeric string name. Numeric strings are rejected so a name can never
/// shadow an integer key in `trigger` lookups.
fn parse_key(key: Option<&serde_json::Value>, index: usize) -> Result<SlotKey, String> {
    match key {
        Some(serde_json::Value::Number(number)) => {
            let value = number
                .as_i64()
                .filter(|value| i32::try_from(*value).is_ok())
                .ok_or_else(|| {
                    format!(
                        "samples[{}]: 'key' must be an integer (e.g. a MIDI note)",
                        index
                    )
                })?;
            Ok(SlotKey::Number(value as i32))
        }
        Some(serde_json::Value::String(name)) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(format!("samples[{}]: 'key' must not be empty", index));
            }
            if name.parse::<i32>().is_ok() {
                return Err(format!(
                    "samples[{}]: numeric key '{}' must be a JSON number, not a string",
                    index, name
                ));
            }
            Ok(SlotKey::Name(name.to_string()))
        }
        Some(_) => Err(format!(
            "samples[{}]: 'key' must be an integer or a name",
            index
        )),
        None => Err(format!("samples[{}] is missing 'key'", index)),
    }
}

/// One playback voice per slot. Retriggering a slot restarts it (per-slot
/// choke); different slots mix freely.
struct Voice {
    sample: Arc<SampleData>,
    position: usize,
    active: bool,
}

impl Voice {
    fn start(&mut self) {
        self.position = 0;
        self.active = self.sample.len() > 0;
    }
}

pub struct SampleKit {
    ctrl: SampleKitControls,
    inputs: inputs::SampleKitInputs,
    outputs: outputs::SampleKitOutputs,
    voices: Vec<Voice>,
    /// Slot keys mirrored as plain integers (`None` for named slots) so the
    /// per-frame trigger path never touches strings.
    numeric_keys: Vec<Option<i32>>,
    /// Per-block scratch, sized once at build: slot gains and staged swaps.
    gains: Vec<f32>,
    swap_scratch: Vec<Option<Arc<SampleData>>>,
    last_trigger_counts: Vec<u64>,
    last_swaps_version: u64,
    last_trigger_input: f32,
}

impl SampleKit {
    pub(crate) fn new_with_controls(
        controls: SampleKitControls,
        samples: Vec<Arc<SampleData>>,
    ) -> Self {
        let slot_count = samples.len();
        let numeric_keys = (0..slot_count)
            .map(|index| match controls.key(index) {
                Some(SlotKey::Number(key)) => Some(key),
                _ => None,
            })
            .collect();
        let voices = samples
            .into_iter()
            .map(|sample| Voice {
                sample,
                position: 0,
                active: false,
            })
            .collect();

        Self {
            ctrl: controls,
            inputs: inputs::SampleKitInputs::new(),
            outputs: outputs::SampleKitOutputs::new(),
            voices,
            numeric_keys,
            gains: vec![1.0; slot_count],
            swap_scratch: (0..slot_count).map(|_| None).collect(),
            last_trigger_counts: vec![0; slot_count],
            last_swaps_version: 0,
            last_trigger_input: 0.0,
        }
    }
}

impl Module for SampleKit {
    fn name(&self) -> &str {
        "SampleKit"
    }

    fn process(&mut self, frames: usize) -> bool {
        let slot_count = self.voices.len();

        // Staged sample swaps (`asset.<i>` control): the lock is taken only
        // when the version counter moved.
        let swaps_version = self.ctrl.swaps_version();
        if swaps_version != self.last_swaps_version {
            self.last_swaps_version = swaps_version;
            self.ctrl.take_swaps(&mut self.swap_scratch);
            for (voice, staged) in self.voices.iter_mut().zip(self.swap_scratch.iter_mut()) {
                if let Some(sample) = staged.take() {
                    // The new sample waits for its next trigger; cutting to
                    // it mid-buffer would click.
                    voice.sample = sample;
                    voice.active = false;
                    voice.position = 0;
                }
            }
        }

        // Control-thread trigger requests, observed once per block.
        for slot in 0..slot_count {
            let count = self.ctrl.trigger_count(slot);
            if count != self.last_trigger_counts[slot] {
                self.last_trigger_counts[slot] = count;
                self.voices[slot].start();
            }
        }

        // Gains are control-rate: read once per block.
        for slot in 0..slot_count {
            self.gains[slot] = self.ctrl.gain(slot);
        }

        for i in 0..frames {
            let trigger = self.inputs.trigger(i);
            if trigger > 0.5 && self.last_trigger_input <= 0.5 {
                let key = self.inputs.key(i);
                if key.is_finite() {
                    let key = key.round();
                    if (i32::MIN as f32..=i32::MAX as f32).contains(&key) {
                        let key = key as i32;
                        if let Some(slot) = self.numeric_keys.iter().position(|k| *k == Some(key)) {
                            self.voices[slot].start();
                        }
                    }
                }
            }
            self.last_trigger_input = trigger;

            let mut left = 0.0;
            let mut right = 0.0;
            for (slot, voice) in self.voices.iter_mut().enumerate() {
                if !voice.active {
                    continue;
                }
                let (l, r) = voice.sample.frame_at(voice.position);
                let gain = self.gains[slot];
                left += l * gain;
                right += r * gain;
                voice.position += 1;
                if voice.position >= voice.sample.len() {
                    voice.active = false;
                    voice.position = 0;
                }
            }
            self.outputs.set(i, left, right);
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
}

#[cfg(test)]
mod tests;
