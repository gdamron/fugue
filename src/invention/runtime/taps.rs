//! Taps on a module's outputs, for live views (see
//! `crate::invention::taps`).

use super::{GraphCommandError, RunningInvention};
use crate::invention::taps::OutputTap;

impl RunningInvention {
    /// Opens a tap on `module_id`'s outputs for a live view: the `ports`
    /// named (one or two), or with none named, its audio (`audio_left` and
    /// `audio_right`, else `audio`, else its first output). Read it with
    /// [`OutputTap::read`] a few times a second or more; it holds under a
    /// second of bins between reads.
    ///
    /// At most [`MAX_OUTPUT_TAPS`](crate::MAX_OUTPUT_TAPS) are open at
    /// once. Dropping the tap closes it, and so does an edit that removes
    /// or rebuilds the module (a swap, or a reload that rebuilds it), or
    /// stopping the invention. The
    /// audio thread sends only min/max bins, never the audio itself, and
    /// opening or closing a tap changes nothing anyone hears.
    pub fn tap_output(
        &self,
        module_id: &str,
        ports: &[&str],
    ) -> Result<OutputTap, GraphCommandError> {
        let mut tap = self.live.tap_output(module_id, ports)?;
        // Its frames are the backend's, whatever rate the modules were
        // built at.
        tap.sample_rate = self.backend.sample_rate();
        Ok(tap)
    }
}
