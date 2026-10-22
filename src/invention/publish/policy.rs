//! The lock the running graph was built under, which live edits build under
//! (see `crate::pkg::lock::BuildPolicy`).

use super::LiveGraph;
use crate::pkg::lock::BuildPolicy;

impl LiveGraph {
    /// The policy the running graph was built under: a live edit builds its
    /// modules under it, not under the session's slot, which a host moves
    /// as a reload commits rather than in the same step. Read it after the
    /// generation an edit plans from, so a reload committing between the
    /// two is refused as the edit publishes; an edit that builds before it
    /// takes a base checks [`Self::policy_moved`] after.
    pub(crate) fn graph_policy(&self) -> BuildPolicy {
        #[cfg(not(target_arch = "wasm32"))]
        return BuildPolicy::of(self.state.lock().unwrap().built_under.clone());
        #[cfg(target_arch = "wasm32")]
        BuildPolicy::default()
    }

    /// Whether the running graph is now governed otherwise than `built`,
    /// the policy an edit built its module under.
    pub(crate) fn policy_moved(&self, built: &BuildPolicy) -> bool {
        !self.graph_policy().same_as(built)
    }
}
