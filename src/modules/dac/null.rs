//! Audio backend that runs a graph with no output device.
//!
//! Hosts that need a *running* invention but no sound — CI, headless
//! measurement harnesses, offline tooling — start the runtime with this
//! instead of [`AudioDriver`](super::AudioDriver). Nothing is rendered and
//! no time passes, but control-side work (control writes, structural edits,
//! snapshots, reload) applies as it would on a device; see [`NullBackend`].

use std::sync::{Arc, Mutex, Weak};

use super::BlockRenderFn;

/// Backend that starts instantly and never pulls audio.
///
/// The render closure owns the graph and its command receiver, so
/// [`NullBackend`] holds it for the lifetime of the run: dropping it would
/// disconnect the command channel and silently strand every subsequent
/// mutation.
///
/// No audio is produced and no time passes musically: the sample count
/// stays where it started. Instead, each change the runtime submits — a
/// control write, a structural edit, an input write — is taken up before
/// the call that submitted it returns, by a zero-length block rendered on
/// the calling thread through the same code the audio thread runs:
///
/// - a control write timed now applies, so it reads back at once (from
///   `get_control`, snapshots and `describe`);
/// - a request timed for a later sample or a later wall-clock time stays
///   pending, and is listed as pending, because that sample never comes;
/// - a structural edit is installed.
///
/// Modules never process, so nothing a module does over time (a
/// scheduler's events, an envelope, a sequencer's steps) happens. A host
/// that needs the graph to actually advance wants a clocked backend
/// instead.
///
/// Deliberately has no `Default`: a backend reporting 0 Hz would misreport the
/// runtime's sample rate rather than fail, so the rate is always explicit.
pub struct NullBackend {
    sample_rate: u32,
    render: Arc<Mutex<Option<BlockRenderFn>>>,
}

impl NullBackend {
    /// Creates a backend reporting `sample_rate` Hz.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            render: Arc::new(Mutex::new(None)),
        }
    }

    /// The handle the runtime settles this backend's graph through.
    pub(crate) fn settle_handle(&self) -> Settle {
        Settle(Arc::downgrade(&self.render))
    }
}

impl Drop for NullBackend {
    /// Takes the graph out under the render lock and drops it off the lock,
    /// as [`stop`](super::AudioBackend::stop) does. A settle in progress
    /// on another thread may hold the render's last strong reference
    /// otherwise, and would then drop the graph while it holds the live
    /// graph's publisher, which the graph's teardown takes.
    fn drop(&mut self) {
        super::AudioBackend::stop(self);
    }
}

impl super::AudioBackend for NullBackend {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self, render: BlockRenderFn) -> Result<(), Box<dyn std::error::Error>> {
        *lock(&self.render) = Some(render);
        Ok(())
    }

    fn stop(&mut self) {
        // The graph drops off the lock, so a settle waiting for it is not
        // held up by sinks finalizing their files.
        let render = lock(&self.render).take();
        drop(render);
    }
}

/// Renders zero-length blocks of a [`NullBackend`]'s graph on the calling
/// thread (see [`Settle::settle`]). Weak, so a handle kept in a control
/// surface never keeps a stopped or dropped backend's graph alive.
#[derive(Clone)]
pub(crate) struct Settle(Weak<Mutex<Option<BlockRenderFn>>>);

impl Settle {
    /// Runs `first`, then renders one zero-length block, which takes up
    /// every change queued before this call and applies the requests due
    /// now (see `SignalGraph::process_block`). Both run under this
    /// backend's render lock, so `first` can make room the render needs
    /// (freeing what earlier renders retired) with no render in between.
    /// Control thread only, and never from inside a render. The render
    /// itself takes no lock (a zero-length block runs no module's
    /// `process`). Waits while another thread settles, so renders never
    /// overlap and the graph passes between threads through this mutex.
    /// Does nothing once the backend has stopped, or after a render
    /// panicked (as a device stream then renders silence).
    pub(crate) fn settle(&self, first: impl FnOnce()) -> bool {
        let Some(render) = self.0.upgrade() else {
            return false;
        };
        let Ok(mut render) = render.lock() else {
            return false;
        };
        let Some(render) = render.as_mut() else {
            return false;
        };
        first();
        render(&mut [], &mut []);
        true
    }
}

/// Locks the render slot, poisoned or not: `start` and `stop` only replace
/// what it holds.
fn lock(render: &Mutex<Option<BlockRenderFn>>) -> std::sync::MutexGuard<'_, Option<BlockRenderFn>> {
    render
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::super::AudioBackend;
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Dropping the backend frees its graph itself, on the dropping thread,
    /// even while a settle elsewhere holds the render slot: that settle
    /// never ends up dropping the graph.
    #[test]
    fn a_dropped_backend_frees_its_graph_while_a_settle_holds_the_slot() {
        struct Flag(Arc<AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = Flag(dropped.clone());
        let mut backend = NullBackend::new(48_000);
        backend
            .start(Box::new(move |_: &mut [f32], _: &mut [f32]| {
                let _ = &flag;
            }))
            .unwrap();
        // A settle in progress elsewhere holds the slot.
        let settling = backend.settle_handle().0.upgrade().unwrap();
        drop(backend);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(lock(&settling).is_none());
    }
}
