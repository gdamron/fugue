//! A test module with asset controls, built by [`SamplerFactory`] as type
//! `sampler`: two slots, `asset.0` and `asset.1`, each holding a sample
//! loaded at the engine rate, as a kit's slots do. Output `slot.N` is slot
//! N's length in engine-rate frames, so a test reads which sample, at which
//! rate, the module holds.

use std::sync::Arc;

use crate::control_request::{ControlCells, ControlDecl, ControlIndex, ControlTable, Refusal};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::ConfigKey;
use crate::modules::sample_loading::{AssetLoader, SampleData, SAMPLE_ASSET};
use crate::payload::{Payload, Retired, Shared};
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

pub(crate) const SAMPLER: &str = "sampler";
const SLOTS: usize = 2;

const DECLS: &[ControlDecl] =
    &[ControlDecl::payload("asset", SAMPLE_ASSET, "A slot's sample").indexed(SLOTS as u16)];

static TABLE: ControlTable = ControlTable::of(DECLS);

pub(crate) struct Sampler {
    slots: [Shared<SampleData>; SLOTS],
    cells: Arc<ControlCells>,
    out: [[f32; MAX_BLOCK]; SLOTS],
}

/// Builds a sampler from `{"samples": [path, path]}`.
pub(crate) struct SamplerFactory;

impl ModuleFactory for SamplerFactory {
    fn type_id(&self) -> &'static str {
        SAMPLER
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[ConfigKey::json("samples")] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let loader = AssetLoader::current(sample_rate);
        let cells = Arc::new(ControlCells::new(
            DECLS.iter().flat_map(|decl| [decl.default; SLOTS]),
        ));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        let mut slots = Vec::with_capacity(SLOTS);
        for slot in 0..SLOTS {
            let asset = config["samples"][slot].as_str().ok_or("two sample paths")?;
            slots.push(Shared::from_arc(loader.load(asset)?));
            surface.show_payload(&format!("asset.{slot}"), asset.into());
        }
        let Ok(slots) = <[_; SLOTS]>::try_from(slots) else {
            unreachable!("one per slot")
        };
        let sampler = Sampler {
            slots,
            cells,
            out: [[0.0; MAX_BLOCK]; SLOTS],
        };
        // A part of another type first: a codec finds its own by type.
        let surface = surface
            .with_context(Arc::new(SLOTS))
            .with_context(Arc::new(loader));
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(sampler)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

impl Module for Sampler {
    fn name(&self) -> &str {
        "Sampler"
    }

    fn process(&mut self, frames: usize) -> bool {
        for (out, slot) in self.out.iter_mut().zip(&self.slots) {
            out[..frames].fill(slot.len() as f32);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &["slot.0", "slot.1"]
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.out[index]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.out[index]
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input '{port}'"))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let slot = self.outputs().iter().position(|out| *out == port);
        slot.map(|slot| self.out[slot][0])
            .ok_or("no such output".into())
    }

    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        let Some(slot) = self.slots.get_mut(usize::from(control.0)) else {
            return Err((Refusal::Unsupported, payload));
        };
        match payload.downcast::<SampleData>() {
            Ok(sample) => Ok(std::mem::replace(slot, sample).into()),
            Err(payload) => Err((Refusal::Invalid, payload)),
        }
    }
}

/// Writes a mono 44.1 kHz WAV of `frames` frames into `dir` as `name`, and
/// returns its path as a sampler's config and writes take it. Use a fresh
/// `dir` per test: the sample cache is keyed by path.
pub(crate) fn wav(dir: &std::path::Path, name: &str, frames: usize) -> String {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..frames {
        writer.write_sample(i16::MAX / 2).unwrap();
    }
    writer.finalize().unwrap();
    path.to_string_lossy().into_owned()
}
