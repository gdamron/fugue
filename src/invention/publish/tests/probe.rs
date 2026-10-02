//! A sink that records which thread drops it, and whether the publisher
//! was locked at the time.

use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use crate::invention::publish::Publisher;
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, SinkModule, MAX_BLOCK};

/// The probe's module type id.
pub(crate) const DROP_PROBE: &str = "drop_probe";

/// Builds probe sinks that record the thread each one is dropped on.
#[derive(Clone, Default)]
pub(crate) struct DropProbeFactory {
    drops: Arc<Mutex<Vec<ThreadId>>>,
    watched: Arc<OnceLock<Arc<Mutex<Publisher>>>>,
    under_lock: Arc<Mutex<Vec<bool>>>,
}

impl DropProbeFactory {
    /// Makes every probe record, when dropped, whether `publisher` was
    /// locked.
    pub(crate) fn watch(&self, publisher: Arc<Mutex<Publisher>>) {
        assert!(self.watched.set(publisher).is_ok(), "already watching");
    }

    /// For each watched probe dropped so far, whether the publisher was
    /// locked when it dropped.
    pub(crate) fn dropped_under_lock(&self) -> Vec<bool> {
        self.under_lock.lock().unwrap().clone()
    }

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
                watched: self.watched.clone(),
                under_lock: self.under_lock.clone(),
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
    watched: Arc<OnceLock<Arc<Mutex<Publisher>>>>,
    under_lock: Arc<Mutex<Vec<bool>>>,
    input: [f32; MAX_BLOCK],
    silence: [f32; MAX_BLOCK],
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.drops.lock().unwrap().push(std::thread::current().id());
        if let Some(publisher) = self.watched.get() {
            // `try_lock` fails while any thread, this one included, holds it.
            let locked = publisher.try_lock().is_err();
            self.under_lock.lock().unwrap().push(locked);
        }
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
