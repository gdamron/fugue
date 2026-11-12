//! Pausing and resuming a running piece without unloading it.

use std::sync::Mutex;

use super::RunningInvention;
use crate::control_request::Transport;
use crate::invention::state::RuntimeState;

impl RunningInvention {
    /// Pauses the piece: the output fades to silence over 10 ms and the
    /// piece then holds where it stands, loaded, with its clocks, module
    /// state, control values and waiting requests kept. Control writes and
    /// edits made while it is paused apply as it holds and are heard once
    /// it resumes. Returns at once; the audio thread takes the request at
    /// its next block. Pausing a paused piece changes nothing.
    pub fn pause(&self) {
        self.request_pause(true);
    }

    /// Resumes a paused piece from where it held, the output fading in over
    /// 10 ms. Resuming a playing piece changes nothing.
    pub fn resume(&self) {
        self.request_pause(false);
    }

    /// Whether the piece is paused, as last requested.
    pub fn is_paused(&self) -> bool {
        self.transport.pause_requested()
    }

    /// A pause or resume that changes what is heard leaves the session's
    /// request log inexact: a replay renders the piece as it played,
    /// without its fades or the silence it held.
    fn request_pause(&self, paused: bool) {
        if request_pause(&self.state, &self.transport, paused) {
            self.live.note_unlogged();
        }
    }
}

/// Requests a pause (or a resume) on `transport` and records it in `state`
/// for the status, under the state's lock, so the two never disagree once
/// the call returns. Returns whether it changed the request. Control
/// thread.
pub(crate) fn request_pause(
    state: &Mutex<RuntimeState>,
    transport: &Transport,
    paused: bool,
) -> bool {
    let mut state = state.lock().unwrap();
    state.paused = paused;
    transport.request_pause(paused)
}
