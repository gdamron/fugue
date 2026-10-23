//! A module swap timed like a control request: built now, landing at its
//! time (see `timed`).

use super::timed::EditTime;
use super::{GraphChange, LiveGraph, BUILD_ATTEMPTS};
use crate::control_request::RequestId;
use crate::invention::runtime::GraphCommandError;

impl LiveGraph {
    /// [`Self::swap_module`], landing at `time` rather than at once, and
    /// refused once `ttl` samples pass first (see [`Self::commit_when`]).
    /// The replacement is built now, against the current registry, and
    /// built again when a commit adopts another one first, as an untimed
    /// swap is.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn swap_module_at(
        &self,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
        preserve_connections: bool,
        time: EditTime<'_>,
        ttl: Option<u64>,
    ) -> Result<RequestId, GraphCommandError> {
        let mut attempt = 1;
        loop {
            let registry = self.registry.current();
            let policy = self.graph_policy();
            let build = || GraphChange::build(&registry, sample_rate, id, module_type, config);
            let built = match policy.run(build) {
                Ok(built) => built,
                Err(_) if attempt < BUILD_ATTEMPTS && !self.registry.is_current(&registry) => {
                    attempt += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut change = self.begin();
            // A reload that committed another policy before `begin` is not
            // caught when the swap publishes; one after it is.
            if self.policy_moved(&policy) {
                if attempt < BUILD_ATTEMPTS {
                    attempt += 1;
                    continue;
                }
                return Err(GraphCommandError::TopologyMoved);
            }
            if !change.contains(id) {
                return Err(GraphCommandError::UnknownModule(id.to_string()));
            }
            if !preserve_connections {
                change.disconnect_module(id);
            }
            change.upsert(id, built);
            let prepared = change.prepare()?;
            // The swap's document change is the module's, which adopting
            // it records (see `commit_mirrors`): nothing more to retain.
            let committed =
                self.commit_built_when(prepared, Some(&registry), time, ttl, |_| {}, &[]);
            match committed {
                // Another commit published, or adopted another registry,
                // since `begin`.
                Err(GraphCommandError::TopologyMoved) if attempt < BUILD_ATTEMPTS => attempt += 1,
                result => return result,
            }
        }
    }
}
