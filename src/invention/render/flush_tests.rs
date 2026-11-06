//! Every render, live or offline, runs its modules with flush-to-zero set,
//! and the DAC's count of silenced non-finite samples reaches the meter.

use super::RenderEngine;
use crate::invention::manual_backend::start_manual;
use crate::{
    flush_to_zero_enabled, GraphModule, Invention, InventionBuilder, Module, ModuleBuildResult,
    ModuleFactory, ModuleRegistry, MAX_BLOCK,
};

const PROBE: &str = "float_mode_probe";
const SUPPORTED: bool = cfg!(any(target_arch = "aarch64", target_arch = "x86_64"));

/// Wires the probe's `flag` output to the left channel and its `nan`
/// output to the right.
const INVENTION: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "probe", "type": "float_mode_probe" },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "probe", "from_port": "flag", "to": "dac", "to_port": "audio_left" },
        { "from": "probe", "from_port": "nan", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

/// Outputs 1.0 on `flag` while its `process` runs with flush-to-zero set
/// (0.0 otherwise), and NaN on `nan`.
struct Probe {
    flag: [f32; MAX_BLOCK],
    nan: [f32; MAX_BLOCK],
}

impl Module for Probe {
    fn name(&self) -> &str {
        "FloatModeProbe"
    }

    fn process(&mut self, frames: usize) -> bool {
        let flag = if flush_to_zero_enabled() { 1.0 } else { 0.0 };
        self.flag[..frames].fill(flag);
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &["flag", "nan"]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }

    fn output_block(&self, index: usize) -> &[f32] {
        if index == 0 {
            &self.flag
        } else {
            &self.nan
        }
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input '{port}'"))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "flag" => Ok(self.flag[0]),
            "nan" => Ok(self.nan[0]),
            _ => Err(format!("no output '{port}'")),
        }
    }
}

struct ProbeFactory;

impl ModuleFactory for ProbeFactory {
    fn type_id(&self) -> &'static str {
        PROBE
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(Probe {
                flag: [0.0; MAX_BLOCK],
                nan: [f32::NAN; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

fn runtime() -> crate::invention::runtime::InventionRuntime {
    let mut registry = ModuleRegistry::default();
    registry.register(ProbeFactory);
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(Invention::from_json(INVENTION).unwrap())
        .unwrap();
    runtime
}

fn expected_flag() -> f32 {
    if SUPPORTED {
        1.0
    } else {
        0.0
    }
}

#[test]
fn a_live_render_runs_modules_with_flush_to_zero_and_meters_silenced_samples() {
    let (running, pump) = start_manual(runtime());
    let (left, right) = pump.render_stereo(3);
    assert!(left.iter().all(|&s| s == expected_flag()), "{left:?}");
    assert!(right.iter().all(|&s| s.to_bits() == 0), "{right:?}");
    // The mode is the render's, not left on the thread that called it.
    assert!(!flush_to_zero_enabled());

    let levels = running.master_meter();
    assert_eq!(levels.non_finite_samples, right.len() as u64);
    let reading = crate::MeterReading::new("master", &levels);
    assert_eq!(reading.non_finite_samples, right.len() as u64);
    // A running total: a later window still carries it.
    pump.render(1);
    assert_eq!(
        running.master_meter().non_finite_samples,
        right.len() as u64 + 64
    );
}

#[test]
fn an_offline_render_runs_modules_with_flush_to_zero() {
    let mut engine = RenderEngine::new(48_000);
    engine.install_runtime(runtime());
    let mut output = [9.0f32; 2 * 300];
    engine.render_interleaved(&mut output).unwrap();
    for frame in output.chunks(2) {
        assert_eq!(frame[0], expected_flag(), "{frame:?}");
        assert_eq!(frame[1].to_bits(), 0, "{frame:?}");
    }
    assert!(!flush_to_zero_enabled());
}
