//! A backend whose blocks a test renders by hand, standing in for the audio
//! thread so publication timing is deterministic. Shared by every test that
//! drives a running invention block by block.

use std::sync::{Arc, Mutex};

use super::runtime::{InventionRuntime, RunningInvention};
use crate::modules::dac::BlockRenderFn;
use crate::modules::AudioBackend;

/// The sample rate the manual backend reports.
pub(crate) const SAMPLE_RATE: u32 = 48_000;

/// The test's end of a [`ManualBackend`]: renders the running invention's
/// blocks on the calling thread.
#[derive(Clone, Default)]
pub(crate) struct Pump(Arc<Mutex<Option<BlockRenderFn>>>);

impl Pump {
    /// Renders `blocks` blocks of 64 frames and returns the left channel.
    pub(crate) fn render(&self, blocks: usize) -> Vec<f32> {
        let mut render = self.0.lock().unwrap();
        let render = render.as_mut().expect("backend started");
        let mut out = Vec::new();
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        for _ in 0..blocks {
            render(&mut left, &mut right);
            out.extend_from_slice(&left);
        }
        out
    }

    /// Renders one block and keeps nothing, so a test can count what the
    /// block alone allocates.
    pub(crate) fn block(&self) {
        let mut render = self.0.lock().unwrap();
        let render = render.as_mut().expect("backend started");
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        render(&mut left, &mut right);
    }

    /// A backend rendering through this pump.
    pub(crate) fn backend(&self) -> ManualBackend {
        ManualBackend(self.clone())
    }
}

/// An [`AudioBackend`] that renders only when its [`Pump`] is told to.
pub(crate) struct ManualBackend(Pump);

impl AudioBackend for ManualBackend {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn start(&mut self, render: BlockRenderFn) -> Result<(), Box<dyn std::error::Error>> {
        *self.0 .0.lock().unwrap() = Some(render);
        Ok(())
    }

    fn stop(&mut self) {
        self.0 .0.lock().unwrap().take();
    }
}

/// Starts `runtime` on a manual backend, returning the pump that renders it.
pub(crate) fn start_manual(runtime: InventionRuntime) -> (RunningInvention, Pump) {
    let pump = Pump::default();
    let running = runtime
        .start_with_backend(pump.backend())
        .expect("a manual backend starts");
    (running, pump)
}

/// A running invention on a manual backend whose control reads first
/// render a block, so a write made before one reads back once the audio
/// thread has applied it, as it would within a block on a live device.
pub(crate) struct Settled {
    pub(crate) running: RunningInvention,
    pub(crate) pump: Pump,
}

impl Settled {
    pub(crate) fn start(runtime: InventionRuntime) -> Self {
        let (running, pump) = start_manual(runtime);
        Self { running, pump }
    }

    /// Renders a block, then reads `key` of `module_id`.
    pub(crate) fn get_control(
        &self,
        module_id: &str,
        key: &str,
    ) -> Result<crate::ControlValue, crate::GraphCommandError> {
        self.pump.block();
        self.running.get_control(module_id, key)
    }
}

impl std::ops::Deref for Settled {
    type Target = RunningInvention;

    fn deref(&self) -> &RunningInvention {
        &self.running
    }
}

impl std::ops::DerefMut for Settled {
    fn deref_mut(&mut self) -> &mut RunningInvention {
        &mut self.running
    }
}
