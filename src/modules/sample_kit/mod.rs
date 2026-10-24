//! Sample kit module: maps a `play` pulse's value (or MIDI note number, or named
//! index) to one of N preloaded sample slots. The canonical drum kit.
//!
//! ```json
//! {
//!   "type": "sample_kit",
//!   "config": {
//!     "samples": [
//!       { "key": 36, "asset": "fugue.drums.808@1.2.0:kick.wav" },
//!       { "key": 38, "asset": "fugue.drums.808@1.2.0:snare.wav" },
//!       { "key": "ride", "asset": { "path": "./samples/ride.wav" }, "level": 0.8 }
//!     ]
//!   }
//! }
//! ```
//!
//! Every slot's buffer is decoded and resampled to the engine rate at build
//! time (or on a control thread, for an `asset.<i>` write), so the audio
//! thread only mixes: one voice per slot, retrigger restarts the slot,
//! distinct slots overlap freely. Every control is declared (see
//! `controls`): writes reach the kit as requests it applies between blocks.

use std::sync::Arc;

use crate::control_request::{
    local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable, Refusal,
    RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{finite_f32, whole_number, ConfigReader};
use crate::modules::sample_loading::{check_entry_fields, SampleData};
use crate::payload::{Payload, Retired, Shared};
use crate::{ControlMeta, Module};

use self::controls::{control_at, KitControl};
pub use self::controls::{SlotKey, SlotSpec};

mod controls;
mod inputs;
mod outputs;

pub struct SampleKitFactory;

const TYPE_ID: &str = "sample_kit";

/// The fields a `samples` entry takes.
const SLOT_FIELDS: &[&str] = &["key", "asset", "level"];

impl ModuleFactory for SampleKitFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        const { &[crate::module_config::ConfigKey::json("samples")] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let specs = parse_config(config)?;
        let numeric_keys = specs
            .iter()
            .map(|spec| match spec.key {
                SlotKey::Number(key) => Some(key),
                SlotKey::Name(_) => None,
            })
            .collect();
        let built = controls::build(sample_rate, specs)?;
        // A recorded `level.N` write is later than the slot's authored
        // level, so it wins; a `key.N` is refused, as a live write is.
        crate::factory::apply_control_keys(&built.surface, config, |key| {
            key.starts_with("level.") || key.starts_with("key.")
        })?;
        let kit = SampleKit::new(built.table, built.cells, built.samples, numeric_keys);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(kit)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(built.surface)),
            sink: None,
        })
    }
}

/// Parses the `samples` slot list. A missing or null config builds an empty
/// kit (module type discovery constructs every type with a null config).
fn parse_config(config: &serde_json::Value) -> Result<Vec<SlotSpec>, String> {
    let reader = ConfigReader::new(TYPE_ID, config);
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

        check_entry_fields(slot, &format!("samples[{index}]"), SLOT_FIELDS)?;

        let key = parse_key(slot.get("key"), index, &reader)?;
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

        let level = match slot.get("level") {
            Some(value) => finite_f32(value).map_err(|r| {
                reader
                    .refuse(&format!("samples[{index}].level"), r)
                    .to_string()
            })?,
            None => 1.0,
        };

        specs.push(SlotSpec { key, asset, level });
    }
    Ok(specs)
}

/// A slot key is a JSON integer (`play` value / MIDI note number) or a
/// non-numeric string name. Numeric strings are rejected so a name can never
/// shadow an integer key in `play` lookups.
fn parse_key(
    key: Option<&serde_json::Value>,
    index: usize,
    reader: &ConfigReader,
) -> Result<SlotKey, String> {
    match key {
        Some(number @ serde_json::Value::Number(_)) => whole_number::<i32>(number)
            .map(SlotKey::Number)
            .map_err(|r| {
                reader
                    .refuse(&format!("samples[{index}].key"), r)
                    .to_string()
            }),
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
    /// What the slot's next play plays.
    sample: Shared<SampleData>,
    /// The sample a swap replaced while it sounded, which rings out rather
    /// than stopping dead (a click). Held until a later swap retires it;
    /// the sample cache holds its buffer anyway.
    previous: Shared<SampleData>,
    /// Whether the voice is playing `previous`.
    ringing_out: bool,
    level: f32,
    position: usize,
    active: bool,
}

impl Voice {
    fn start(&mut self) {
        self.ringing_out = false;
        self.position = 0;
        self.active = self.sample.len() > 0;
    }

    fn sounding(&self) -> &SampleData {
        match self.ringing_out {
            true => &self.previous,
            false => &self.sample,
        }
    }

    /// Takes `sample` for the next play, handing back the handle it no
    /// longer needs: the sounding one rings out to its end.
    fn swap(&mut self, sample: Shared<SampleData>) -> Shared<SampleData> {
        let replaced = std::mem::replace(&mut self.sample, sample);
        if !self.active || self.ringing_out {
            return replaced;
        }
        self.ringing_out = true;
        std::mem::replace(&mut self.previous, replaced)
    }
}

pub struct SampleKit {
    table: ControlTable,
    cells: Arc<ControlCells>,
    inputs: inputs::SampleKitInputs,
    outputs: outputs::SampleKitOutputs,
    voices: Vec<Voice>,
    /// Slot keys mirrored as plain integers (`None` for named slots) so the
    /// per-frame play path never touches strings.
    numeric_keys: Vec<Option<i32>>,
    last_play_input: f32,
}

impl SampleKit {
    fn new(
        table: ControlTable,
        cells: Arc<ControlCells>,
        samples: Vec<Shared<SampleData>>,
        numeric_keys: Vec<Option<i32>>,
    ) -> Self {
        let slots = samples.len();
        let voices = (samples.into_iter().enumerate())
            .map(|(slot, sample)| {
                // The authored level; a write made before the build is
                // applied as the kit binds.
                let level = match cells.load(controls::index_of(KitControl::Level(slot), slots)) {
                    Some(RtValue::F32(level)) => level,
                    _ => 1.0,
                };
                Voice {
                    previous: sample.clone(),
                    sample,
                    ringing_out: false,
                    level,
                    position: 0,
                    active: false,
                }
            })
            .collect();
        Self {
            table,
            cells,
            inputs: inputs::SampleKitInputs::new(),
            outputs: outputs::SampleKitOutputs::new(),
            voices,
            numeric_keys,
            last_play_input: 0.0,
        }
    }

    fn control(&self, index: ControlIndex) -> Option<KitControl> {
        control_at(index, self.voices.len())
    }
}

impl Module for SampleKit {
    fn name(&self) -> &str {
        "SampleKit"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let play = self.inputs.play(i);
            if play > 0.5 && self.last_play_input <= 0.5 {
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
            self.last_play_input = play;

            let mut left = 0.0;
            let mut right = 0.0;
            for voice in self.voices.iter_mut() {
                if !voice.active {
                    continue;
                }
                let sample = voice.sounding();
                let (l, r) = sample.frame_at(voice.position);
                let len = sample.len();
                left += l * voice.level;
                right += r * voice.level;
                voice.position += 1;
                if voice.position >= len {
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

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&self.table, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (self.control(control), value) {
            (Some(KitControl::Play), RtValue::I32(slot)) => {
                let voice = usize::try_from(slot)
                    .ok()
                    .and_then(|slot| self.voices.get_mut(slot));
                voice.ok_or(Refusal::Invalid)?.start();
                Ok(value)
            }
            (Some(KitControl::Level(slot)), RtValue::F32(level)) => {
                let voice = &mut self.voices[slot];
                voice.level = level.clamp(0.0, 2.0);
                Ok(RtValue::F32(voice.level))
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    /// A slot's new sample waits for its next play, and the one sounding
    /// rings out: cutting either mid-buffer would click.
    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        let Some(KitControl::Asset(slot)) = self.control(control) else {
            return Err((Refusal::Unsupported, payload));
        };
        let sample = payload
            .downcast::<SampleData>()
            .map_err(|payload| (Refusal::Invalid, payload))?;
        Ok(self.voices[slot].swap(sample).into())
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
