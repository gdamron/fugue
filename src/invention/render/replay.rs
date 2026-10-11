//! Replaying a live session's request log in an offline render.
//!
//! Each logged request applies at its recorded sample, by splitting the
//! render there, exactly as the live drain split the block it applied in,
//! and through the same apply a live request takes (automation still
//! waiting first, then the module's declared control). Replay is by sample:
//! a render of the invention as loaded, at the session's sample rate and
//! block grid, then reproduces what played (see
//! [`RequestLog::is_exact`](crate::RequestLog::is_exact)). A log entry's
//! musical time is kept for the record; replaying against a changed tempo
//! by beats is not done here.

use indexmap::IndexMap;

use crate::control_request::{ControlIndex, RtValue};
use crate::invention::graph::SignalGraph;
use crate::invention::runtime::ControlSurfaceInstance;
use crate::RequestLog;

/// A logged request resolved against the render's surfaces.
struct Due {
    sample: u64,
    module_id: String,
    control: ControlIndex,
    value: RtValue,
}

/// The requests a render still has to apply, in log order.
#[derive(Default)]
pub(super) struct Replay {
    due: Vec<Due>,
    next: usize,
    /// Requests the render's graph refused (it would not have logged them).
    pub(super) refused: usize,
}

impl Replay {
    /// Resolves every request in `log` to a declared control of a module
    /// in `surfaces`, or fails naming the first that is not one.
    pub(super) fn resolve(
        log: &RequestLog,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
    ) -> Result<Self, String> {
        let due = log
            .requests
            .iter()
            .map(|request| {
                let control = surfaces
                    .get(&request.module_id)
                    .and_then(|surface| surface.declaration(&request.key))
                    .ok_or_else(|| {
                        format!(
                            "the logged request for '{}' of '{}' names no declared control",
                            request.key, request.module_id
                        )
                    })?
                    .index;
                Ok(Due {
                    sample: request.sample,
                    module_id: request.module_id.clone(),
                    control,
                    value: request.value.into(),
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            due,
            next: 0,
            refused: 0,
        })
    }

    /// Applies every request due at `graph`'s current sample (one already
    /// past applies now, as a late request would), and returns how many of
    /// the next `frames` frames to render before the next is due.
    pub(super) fn apply_due(&mut self, graph: &mut SignalGraph, frames: usize) -> usize {
        let now = graph.current_sample;
        while let Some(due) = self.due.get(self.next).filter(|due| due.sample <= now) {
            if graph
                .apply_control(&due.module_id, due.control, due.value)
                .is_err()
            {
                self.refused += 1;
            }
            self.next += 1;
        }
        self.due
            .get(self.next)
            .map_or(frames, |due| (due.sample - now).min(frames as u64) as usize)
    }
}

#[cfg(test)]
mod tests;
