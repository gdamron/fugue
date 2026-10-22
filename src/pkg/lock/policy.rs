//! The lock a build was governed by, kept to govern what it loads later.
//!
//! A module that loads package content as its controls are written (a
//! sample player's `asset`, say) loads under the lock its graph was built
//! under, captured as it builds, not under the session's slot at the time
//! of the write. The slot changes as a host commits a reload, and not at
//! the instant the graph does: a timed reload's graph plays before the host
//! adopts its lock, and an untimed reload's lock may be the session's a
//! moment before or after its graph is. A live write, a script's included,
//! therefore always loads under the policy of the graph it writes to, and
//! a graph built under a lockfile is never fed bytes that lockfile did not
//! verify. Live structural edits build under the running graph's policy
//! for the same reason (see `LiveGraph::graph_policy`).

#[cfg(not(target_arch = "wasm32"))]
use super::frozen::{govern_here, governing, same_governance, Governance};

/// The lock, if any, that governed a build.
#[derive(Clone, Default)]
pub(crate) struct BuildPolicy(#[cfg(not(target_arch = "wasm32"))] Governance);

impl BuildPolicy {
    /// The lock governing this thread now (see `governing`).
    pub(crate) fn current() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        return Self(governing());
        #[cfg(target_arch = "wasm32")]
        Self()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn of(lock: Governance) -> Self {
        Self(lock)
    }

    /// Runs `body` with this policy governing this thread's resolutions.
    pub(crate) fn run<T>(&self, body: impl FnOnce() -> T) -> T {
        #[cfg(not(target_arch = "wasm32"))]
        return govern_here(self.0.clone(), body);
        #[cfg(target_arch = "wasm32")]
        body()
    }

    /// Whether `other` governs as this does: the same lock, or the same
    /// lockfile over the same packages dir, or neither governed.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return same_governance(&self.0, &other.0);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = other;
            true
        }
    }
}
