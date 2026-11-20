//! A backend the preview pulls blocks from on its own thread, as fast as it
//! likes: no device, no clock, nothing shared with any other runtime.

use std::sync::{Arc, Mutex};

use crate::modules::dac::BlockRenderFn;
use crate::modules::AudioBackend;
use crate::MAX_BLOCK;

/// The preview's end of a [`PullBackend`]: renders the private runtime's
/// blocks on the calling thread and throws the audio away.
pub(super) struct Pull {
    render: Arc<Mutex<Option<BlockRenderFn>>>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Pull {
    /// Renders `frames` frames (at most [`MAX_BLOCK`]).
    pub(super) fn render(&mut self, frames: usize) {
        let frames = frames.min(MAX_BLOCK);
        let mut render = self.render.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(render) = render.as_mut() {
            render(&mut self.left[..frames], &mut self.right[..frames]);
        }
    }
}

/// An [`AudioBackend`] that renders only when its [`Pull`] asks.
pub(super) struct PullBackend {
    sample_rate: u32,
    render: Arc<Mutex<Option<BlockRenderFn>>>,
}

impl PullBackend {
    /// A backend at `sample_rate` Hz and the end that pulls from it.
    pub(super) fn new(sample_rate: u32) -> (Self, Pull) {
        let render = Arc::new(Mutex::new(None));
        let pull = Pull {
            render: Arc::clone(&render),
            left: vec![0.0; MAX_BLOCK],
            right: vec![0.0; MAX_BLOCK],
        };
        (
            Self {
                sample_rate,
                render,
            },
            pull,
        )
    }
}

impl AudioBackend for PullBackend {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self, render: BlockRenderFn) -> Result<(), Box<dyn std::error::Error>> {
        *self.render.lock().unwrap_or_else(|e| e.into_inner()) = Some(render);
        Ok(())
    }

    fn stop(&mut self) {
        let render = self.render.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(render);
    }
}
