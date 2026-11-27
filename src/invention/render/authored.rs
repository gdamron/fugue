//! The render's retained document read with its authored epoch, for a host
//! that keeps its own revision of the document, as the in-chat player does.

use super::RenderEngine;
use crate::Invention;

impl RenderEngine {
    /// [`Self::document`] and the authored epoch it holds every recorded
    /// write up to, read together, as
    /// [`RunningInvention::document_with_epoch`](crate::RunningInvention::document_with_epoch)
    /// gives them. The epoch moves once for each authored write recorded,
    /// so a host comparing it around a call knows whether the call recorded
    /// any; the document holds what was recorded, but for an agent's prompt
    /// written over its definition's, which plays unrecorded until a reload.
    /// A performed write, a write to an event control and one that failed
    /// as it was made record nothing.
    pub fn document_with_epoch(&self) -> (Option<Invention>, u64) {
        let state = self.state.lock().unwrap();
        (state.document(), state.authored_epoch)
    }
}
