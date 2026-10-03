//! A sink that records which thread drops it.

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, SinkModule, MAX_BLOCK};

/// The probe's module type id.
pub(crate) const DROP_PROBE: &str = "drop_probe";

/// Builds probe sinks that record the thread each one is dropped on.
#[derive(Clone, Default)]
pub(crate) struct DropProbeFactory {
    drops: Arc<Mutex<Vec<ThreadId>>>,
}

impl DropProbeFactory {
    /// Waits up to `timeout` for `count` probes to drop, returning the
    /// threads they dropped on so far.
    pub(crate) fn wait_for_drops(&self, count: usize, timeout: Duration) -> Vec<ThreadId> {
        let deadline = Instant::now() + timeout;
        loop {
            let drops = self.drops.lock().unwrap().clone();
            if drops.len() >= count || Instant::now() >= deadline {
                return drops;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl ModuleFactory for DropProbeFactory {
    fn type_id(&self) -> &'static str {
        DROP_PROBE
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        Ok(ModuleBuildResult {
            module: GraphModule::Sink(Box::new(DropProbe {
                drops: self.drops.clone(),
                input: [0.0; MAX_BLOCK],
                silence: [0.0; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: None,
            sink: Some(()),
        })
    }

    fn is_sink(&self) -> bool {
        true
    }
}

struct DropProbe {
    drops: Arc<Mutex<Vec<ThreadId>>>,
    input: [f32; MAX_BLOCK],
    silence: [f32; MAX_BLOCK],
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.drops.lock().unwrap().push(std::thread::current().id());
    }
}

impl Module for DropProbe {
    fn name(&self) -> &str {
        "DropProbe"
    }

    fn process(&mut self, _frames: usize) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        &["audio"]
    }

    fn outputs(&self) -> &[&str] {
        &[]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.input
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &[]
    }

    fn set_input(&mut self, _port: &str, _value: f32) -> Result<(), String> {
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output '{port}'"))
    }
}

impl SinkModule for DropProbe {
    fn sink_block(&self) -> (&[f32], &[f32]) {
        (&self.silence, &self.silence)
    }
}
