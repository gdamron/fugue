//! What an in-chat player's views read between buffers: the output levels
//! and the beats a clock has counted, in one call that neither allocates
//! nor frees, so a player can ask from its audio thread many times a second.

use crate::ControlValue;

use super::{OfflineLevels, RenderEngine};

/// One read of the views: the output levels since the last read, as
/// [`RenderEngine::take_levels`] returns them, and the beats counted by the
/// clock asked about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewReading {
    /// The output's levels since the last read of levels, which then start
    /// over.
    pub levels: OfflineLevels,
    /// The clock's `elapsed` control, in beats; `None` when no clock was
    /// named or the piece has no module of that id with an `elapsed` count.
    pub beats: Option<f64>,
}

impl RenderEngine {
    /// Takes the output levels (as [`Self::take_levels`] does) and reads the
    /// beats `clock` has counted; `None` with nothing loaded. Neither
    /// allocates nor frees, whether the clock exists or not. Read between
    /// renders, never during one.
    pub fn take_view_reading(&mut self, clock: Option<&str>) -> Option<ViewReading> {
        let levels = self.take_levels()?;
        let beats = clock.and_then(|id| self.elapsed_beats(id));
        Some(ViewReading { levels, beats })
    }

    /// The `elapsed` control of module `id`, without the error message
    /// [`Self::get_control`] builds for a module that is missing. (A module
    /// that exists but counts no beats still answers with a message, so a
    /// player names only a clock.)
    fn elapsed_beats(&self, id: &str) -> Option<f64> {
        let surfaces = self.control_surfaces.lock().ok()?;
        match surfaces.get(id)?.get_control("elapsed") {
            Ok(ControlValue::Number(beats)) => Some(f64::from(beats)),
            _ => None,
        }
    }
}
