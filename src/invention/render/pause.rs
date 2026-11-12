//! Pausing and resuming an offline render as a live piece pauses.

use super::RenderEngine;
use crate::invention::runtime::request_pause;

impl RenderEngine {
    /// Pauses the loaded piece as [`RunningInvention::pause`] does: the
    /// next frames rendered fade to silence over 10 ms, and every frame
    /// after them is silence, with the piece held where it stood until
    /// [`Self::resume`]. Does nothing with nothing loaded; a new load
    /// plays.
    ///
    /// [`RunningInvention::pause`]: crate::RunningInvention::pause
    pub fn pause(&self) {
        self.request_pause(true);
    }

    /// Resumes a paused piece, fading in over 10 ms. Resuming a playing
    /// piece changes nothing.
    pub fn resume(&self) {
        self.request_pause(false);
    }

    /// Whether the loaded piece is paused; false with nothing loaded.
    pub fn is_paused(&self) -> bool {
        self.graph
            .as_ref()
            .is_some_and(|graph| graph.lock().unwrap().transport.pause_requested())
    }

    fn request_pause(&self, paused: bool) {
        if let Some(graph) = &self.graph {
            let transport = graph.lock().unwrap().transport.clone();
            request_pause(&self.state, &transport, paused);
        }
    }
}
