//! The module registry a live graph's edits build against.

use std::sync::{Arc, Mutex};

use crate::ModuleRegistry;

/// The registry live edits build modules against, shared by every clone of
/// a live graph, so a script or agent that outlives a reload builds against
/// what the reload adopted.
///
/// It changes only in a commit step, while the publisher is held (see
/// [`super::LiveGraph::commit_adopting`]), so an edit holding the publisher
/// sees it stay put. Lock order is publisher, then this lock; a reader takes
/// this lock alone, briefly, and never holds it while taking another. Never
/// touched on the audio thread.
#[derive(Clone)]
pub(super) struct LiveRegistry(Arc<Mutex<Arc<ModuleRegistry>>>);

impl LiveRegistry {
    pub(super) fn new(registry: Arc<ModuleRegistry>) -> Self {
        Self(Arc::new(Mutex::new(registry)))
    }

    /// The registry edits build against now.
    pub(super) fn current(&self) -> Arc<ModuleRegistry> {
        self.0.lock().unwrap().clone()
    }

    /// Whether `registry` is still the one edits build against.
    pub(super) fn is_current(&self, registry: &Arc<ModuleRegistry>) -> bool {
        Arc::ptr_eq(&self.0.lock().unwrap(), registry)
    }

    /// Replaces the registry, returning the previous one for the caller to
    /// drop off the publisher's lock. Call only while holding the publisher.
    pub(super) fn replace(&self, registry: Arc<ModuleRegistry>) -> Arc<ModuleRegistry> {
        std::mem::replace(&mut self.0.lock().unwrap(), registry)
    }
}
