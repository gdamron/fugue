//! Writing a logged payload again through its control's front door.

use crate::invention::declared::WriteOptions;
use crate::{ControlValue, RenderEngine};

impl RenderEngine {
    /// Writes the payload the replay stopped at as its text, through its
    /// control's front door, as a front door writes an offline render: its
    /// codec builds the payload here, on the control thread, and the graph
    /// keeps it at once. Call it off the graph's lock, which the write
    /// takes. One its control refuses, or loses to a hold, counts refused.
    pub(in crate::invention::render) fn replay_payload(&mut self) {
        let Some((module_id, key, text)) = self.replay.take_payload() else {
            return;
        };
        // Off the directory's lock too: a scheduler's `schedule` write
        // re-resolves its targets against it.
        let surface = self
            .control_surfaces
            .lock()
            .unwrap()
            .get(&module_id)
            .cloned();
        let written = surface.map(|surface| {
            surface.submit(&key, ControlValue::String(text), &WriteOptions::default())
        });
        if !matches!(written, Some(Ok(queued)) if !queued.lost) {
            self.replay.refused += 1;
        }
    }
}
