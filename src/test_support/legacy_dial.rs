//! A test module on the legacy control path, built by [`LegacyDialFactory`]
//! as type `legacy_dial`: plain atomics behind a [`ControlSurface`] that
//! writes them directly. It stands in for a catalog module while any is
//! still legacy, so the tests of the legacy path need not name one.
//!
//! `rate` is a number clamped to 0.001..=100 (default 1) and `waveform` a
//! string among [`WAVEFORMS`], which a schedule cannot write.

use std::sync::Arc;

use crate::atomic::AtomicF32;
use crate::control_request::{scheduled_number, LegacyRefusal, TableValue};
use crate::traits::{check_listed_control, ControlSurfaceMap};
use crate::{
    ControlMeta, ControlSurface, ControlValue, GraphModule, Module, ModuleBuildResult,
    ModuleFactory, MAX_BLOCK,
};

pub(crate) const LEGACY_DIAL: &str = "legacy_dial";
pub(crate) const WAVEFORMS: [&str; 4] = ["sine", "square", "sawtooth", "triangle"];

fn waveform_position(name: &str) -> Result<usize, String> {
    let position = WAVEFORMS.iter().position(|option| *option == name);
    position.ok_or_else(|| format!("Unknown waveform: {name}"))
}

#[derive(Clone)]
pub(crate) struct LegacyDialControls {
    rate: Arc<AtomicF32>,
    waveform: Arc<AtomicF32>,
}

impl LegacyDialControls {
    fn set_rate(&self, value: f32) {
        self.rate.store(value.clamp(0.001, 100.0));
    }
}

impl ControlSurface for LegacyDialControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("rate", "Rate in Hz")
                .with_range(0.001, 100.0)
                .with_default(self.rate.load()),
            ControlMeta::string("waveform", "Waveform")
                .with_default(WAVEFORMS[self.waveform.load() as usize])
                .with_options(WAVEFORMS.iter().map(|name| name.to_string()).collect()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "rate" => Ok(self.rate.load().into()),
            "waveform" => Ok(WAVEFORMS[self.waveform.load() as usize].into()),
            _ => Err(format!("Unknown control: {key}")),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "rate" => self.set_rate(value.as_number()?),
            "waveform" => {
                self.waveform
                    .store(waveform_position(value.as_string()?)? as f32);
            }
            _ => return Err(format!("Unknown control: {key}")),
        }
        Ok(())
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        match key {
            "waveform" => waveform_position(value.as_string()?).map(drop),
            _ => check_listed_control(&self.controls(), key, value),
        }
    }

    #[allow(private_interfaces)]
    fn set_scheduled(&self, key: &str, value: TableValue) -> Result<(), LegacyRefusal> {
        let value = scheduled_number(value)?;
        match key {
            "rate" => self.set_rate(value),
            _ => return Err(LegacyRefusal::Unsupported),
        }
        Ok(())
    }

    #[allow(private_interfaces)]
    fn get_scheduled(&self, key: &str) -> Result<f32, LegacyRefusal> {
        match key {
            "rate" => Ok(self.rate.load()),
            _ => Err(LegacyRefusal::Unsupported),
        }
    }
}

/// A module with no inputs and one silent output, owning its controls.
struct LegacyDial {
    out: [f32; MAX_BLOCK],
}

pub(crate) struct LegacyDialFactory;

impl ModuleFactory for LegacyDialFactory {
    fn type_id(&self) -> &'static str {
        LEGACY_DIAL
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let controls = LegacyDialControls {
            rate: Arc::new(AtomicF32::new(1.0)),
            waveform: Arc::new(AtomicF32::new(0.0)),
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(LegacyDial {
                out: [0.0; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

impl Module for LegacyDial {
    fn name(&self) -> &str {
        "LegacyDial"
    }

    fn process(&mut self, _frames: usize) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &["out"]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.out
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.out
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input '{port}'"))
    }

    fn get_output(&self, _port: &str) -> Result<f32, String> {
        Ok(0.0)
    }
}
