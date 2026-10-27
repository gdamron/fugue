//! A test module whose controls are undeclared host state, built by
//! [`HostDialFactory`] as type `host_dial`: values a control thread keeps
//! behind a [`ControlSurface`] that writes them directly, as an agent's
//! prompt is kept. No request carries them, and no schedule can write
//! them.
//!
//! `rate` is a number clamped to 0.001..=100 (default 1) and `waveform` a
//! string among [`WAVEFORMS`].

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::traits::{check_listed_control, ControlSurfaceMap};
use crate::{
    ControlMeta, ControlSurface, ControlValue, GraphModule, Module, ModuleBuildResult,
    ModuleFactory, MAX_BLOCK,
};

pub(crate) const HOST_DIAL: &str = "host_dial";
pub(crate) const WAVEFORMS: [&str; 4] = ["sine", "square", "sawtooth", "triangle"];

fn waveform_position(name: &str) -> Result<usize, String> {
    let position = WAVEFORMS.iter().position(|option| *option == name);
    position.ok_or_else(|| format!("Unknown waveform: {name}"))
}

#[derive(Clone)]
pub(crate) struct HostDialControls {
    rate: Arc<AtomicU32>,
    waveform: Arc<AtomicUsize>,
}

impl HostDialControls {
    fn set_rate(&self, value: f32) {
        let rate = value.clamp(0.001, 100.0);
        self.rate.store(rate.to_bits(), Ordering::Relaxed);
    }

    fn rate(&self) -> f32 {
        f32::from_bits(self.rate.load(Ordering::Relaxed))
    }

    fn waveform(&self) -> &'static str {
        WAVEFORMS[self.waveform.load(Ordering::Relaxed)]
    }
}

impl ControlSurface for HostDialControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("rate", "Rate in Hz")
                .with_range(0.001, 100.0)
                .with_default(self.rate()),
            ControlMeta::string("waveform", "Waveform")
                .with_default(self.waveform())
                .with_options(WAVEFORMS.iter().map(|name| name.to_string()).collect()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "rate" => Ok(self.rate().into()),
            "waveform" => Ok(self.waveform().into()),
            _ => Err(format!("Unknown control: {key}")),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "rate" => self.set_rate(value.as_number()?),
            "waveform" => {
                let position = waveform_position(value.as_string()?)?;
                self.waveform.store(position, Ordering::Relaxed);
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
}

/// A module with no inputs and one silent output, owning its controls.
struct HostDial {
    out: [f32; MAX_BLOCK],
}

pub(crate) struct HostDialFactory;

impl ModuleFactory for HostDialFactory {
    fn type_id(&self) -> &'static str {
        HOST_DIAL
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let controls = HostDialControls {
            rate: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            waveform: Arc::new(AtomicUsize::new(0)),
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(HostDial {
                out: [0.0; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

impl Module for HostDial {
    fn name(&self) -> &str {
        "HostDial"
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
