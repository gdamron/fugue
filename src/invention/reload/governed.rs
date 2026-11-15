//! A reload under a lockfile that governs package resolution (see
//! [`crate::pkg::lock`]).

use super::{ReloadPlan, ReloadPolicy, ValidatedDocument};
use crate::pkg::lock::{changes_policy, govern, govern_here, Governance};

/// A candidate's lock: it governs preparation on this thread, and the
/// session once the commit succeeds.
pub(super) struct Candidate(pub(super) Governance);

impl ReloadPolicy for Candidate {
    fn prepare(&mut self, body: &mut dyn FnMut()) {
        govern_here(self.0.clone(), body)
    }

    fn adopt(&mut self) {
        govern(self.0.clone());
    }
}

/// Module types that load package content (samples, and a slicer's
/// sidecar) as they build or as their controls are written.
const PACKAGE_LOADERS: [&str; 4] = [
    "sample_player",
    "sample_kit",
    "sample_instrument",
    "sample_slicer",
];

/// Under another policy than the graph was built under (`built_under`:
/// another lockfile or none), a module that may hold package content is
/// rebuilt rather than kept: what it holds was loaded under that policy, and
/// never verified against the candidate's. That is a package loader, or an
/// instance of a development whose definition instantiates one, directly or
/// through a development it uses (an alias passes a package ref to a loader
/// unchanged, so its config does not change with the policy). Everything
/// else keeps playing, development instances that load nothing included:
/// their definitions were read again under the candidate's lock, and an
/// unchanged one built what is running. Runs as a timed or untimed reload
/// plans, under the candidate's lock ([`govern_here`]).
pub(super) fn rebuild_under_another_policy(
    plan: &mut ReloadPlan,
    validated: &ValidatedDocument,
    built_under: &Governance,
) {
    if !changes_policy(built_under) {
        return;
    }
    let loading = validated.definitions.loaded.instantiating(&PACKAGE_LOADERS);
    let holds_packages =
        |module_type: &str| PACKAGE_LOADERS.contains(&module_type) || loading.contains(module_type);
    let rebuilt: Vec<_> = (validated.resolved.modules.iter())
        .filter(|spec| plan.unchanged.contains(&spec.id) && holds_packages(&spec.module_type))
        .cloned()
        .collect();
    let is_rebuilt = |id: &String| rebuilt.iter().any(|spec| spec.id == *id);
    plan.unchanged.retain(|id| !is_rebuilt(id));
    plan.control_updates.retain(|(id, ..)| !is_rebuilt(id));
    plan.refreshed_configs.retain(|(id, _)| !is_rebuilt(id));
    plan.swapped.extend(rebuilt);
}
