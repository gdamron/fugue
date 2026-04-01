//! wasm stub audio backend.

use crate::SinkOutput;

/// Returns a conventional Web Audio sample rate.
pub fn default_sample_rate() -> Result<u32, Box<dyn std::error::Error>> {
    Ok(48_000)
}

/// Trait for audio output backends.
pub trait AudioBackend: Send {
    fn sample_rate(&self) -> u32;

    fn start(
        &mut self,
        _sample_fn: Box<dyn FnMut() -> SinkOutput + Send>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn stop(&mut self);
}

/// Stub backend for wasm builds.
pub struct AudioDriver {
    sample_rate: u32,
}

impl AudioDriver {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            sample_rate: default_sample_rate()?,
        })
    }
}

impl AudioBackend for AudioDriver {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(
        &mut self,
        _sample_fn: Box<dyn FnMut() -> SinkOutput + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Err("AudioDriver is not available on wasm32; use RenderEngine instead".into())
    }

    fn stop(&mut self) {}
}
