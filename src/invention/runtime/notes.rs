//! The notes a running invention plays, and where its clocks are, for
//! views (see `crate::invention::notes`).

use super::RunningInvention;
use crate::invention::notes::{NoteChanges, Playhead};
use crate::{ControlValue, TimelineContext};

impl RunningInvention {
    /// Drains the note starts and ends the audio thread reported since the
    /// last call, paired into notes, in the order they happened.
    ///
    /// Meant for one reader, which keeps what it needs (the daemon keeps a
    /// bounded history per module): each change is handed out once. The
    /// audio thread holds about a thousand starts and ends between two
    /// calls; past that it drops them and [`NoteChanges::missed`] says how
    /// many, so call it a few times a second or more. Never blocks the
    /// audio thread.
    pub fn take_notes(&self) -> NoteChanges {
        self.live.notes.take()
    }

    /// Where `module_id`'s notes are now: the clock its notes count beats
    /// on (its timeline context, as installed), and that clock's `elapsed`
    /// control, read back as of the latest block. `None` when the module
    /// has no clock a request can name, or is not running.
    pub fn playhead(&self, module_id: &str) -> Option<Playhead> {
        let clock = match self.live.timeline_of(module_id)? {
            TimelineContext::Connected { clock, .. } | TimelineContext::Fallback { clock } => clock,
            TimelineContext::Enclosed { .. } => return None,
        };
        let ControlValue::Number(beats) = self.get_control(&clock, "elapsed").ok()? else {
            return None;
        };
        Some(Playhead {
            clock,
            beats: f64::from(beats),
        })
    }
}
