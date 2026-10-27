//! Planning a batch's change to the running graph from the authored
//! document, and refusing a plan that reaches past the batch.
//!
//! A batch is planned with the same diff reload uses ([`plan_reload`]), but
//! from the retained authored document to the candidate, both as authored.
//! Reload diffs against the configs the runtime built its modules from,
//! which have their assets resolved; the candidate does not. Planned that
//! way, a batch would see every asset-backed module as a change of its own,
//! and the guard below would refuse it. Planning from the document sees only
//! what the batch changed. Reload keeps its own inputs: it diffs resolved
//! against resolved, so it also sees an asset file that changed on disk.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::invention::authored_document::{self, forget_replaced};
use crate::invention::edits::EditedCandidate;
use crate::invention::reload::{plan_reload, ReloadPlan};
use crate::invention::state::{RuntimeModuleInfo, RuntimeState};
use crate::module_config::replaced_by;
use crate::rpc::{RpcError, RpcErrorCode};
use crate::ModuleRegistry;

/// Plans the change from the running graph, as `state` describes it, to
/// `candidate`, and refuses a plan that would touch a module the batch does
/// not name.
///
/// Every module in `candidate.replaced` is planned as rebuilt, even when it
/// came back identical. The plan carries no control updates: the batch's
/// own writes ([`EditedCandidate::final_writes`]) are the only ones a commit
/// makes, so a value is never written twice.
pub(super) fn plan_edits(
    state: &RuntimeState,
    candidate: &EditedCandidate,
    registry: &ModuleRegistry,
) -> Result<ReloadPlan, RpcError> {
    let authored: HashMap<&str, &serde_json::Value> = state
        .document
        .iter()
        .flat_map(|document| &document.modules)
        .map(|spec| (spec.id.as_str(), &spec.config))
        .collect();
    // A write recorded inside a config entry (a sample's `asset.N`, see
    // `authored_document::entry_asset_mut`) changes a non-scalar key, which
    // the diff could only plan as a rebuild. It is the batch's own write, so
    // the configs planned from take it too and it stays a control write.
    let mut current: IndexMap<String, RuntimeModuleInfo> = state
        .modules
        .iter()
        .map(|(id, info)| {
            let mut info = info.clone();
            if let Some(config) = authored.get(id.as_str()) {
                info.config = (*config).clone();
            }
            // What the batch's writes replace is theirs to drop, as the
            // candidate's document already has.
            for write in &candidate.control_writes {
                if write.write.module_id == *id {
                    let keys = registry.config_keys(&info.module_type);
                    forget_replaced(&mut info.config, replaced_by(keys, &write.write.key));
                }
            }
            (id.clone(), info)
        })
        .collect();
    for candidate in &candidate.control_writes {
        let write = &candidate.write;
        if let Some(info) = current.get_mut(&write.module_id) {
            let asset = authored_document::control_json(&write.value);
            authored_document::write_entry_asset(&mut info.config, &write.key, &asset);
        }
    }

    // A survivor's config differs from the document's only by the batch's
    // own writes, each already checked against the module's controls, so
    // only those keys may land as control updates. Any other difference
    // plans a rebuild, which the guard refuses.
    let written: HashSet<(&str, &str)> = candidate
        .control_writes
        .iter()
        .map(|write| (write.write.module_id.as_str(), write.write.key.as_str()))
        .collect();
    let mut plan = plan_reload(
        &current,
        &state.connections,
        &candidate.document,
        &HashSet::new(),
        |module_type| {
            let keys = registry.config_keys(module_type);
            (keys, registry.reload_keys(module_type))
        },
        |_, _| None,
        // A batch's event writes fire without landing in the document.
        |_, _| false,
        |module_id, key| written.contains(&(module_id, key)),
        // The candidate states every write the document records, and a
        // batch removes no key from a module it keeps.
        None,
    )
    .map_err(|error| {
        RpcError::new(
            RpcErrorCode::ModuleBuildFailed,
            format!("the edited invention could not be planned: {error}; nothing was applied"),
        )
    })?;
    guard(&plan, candidate)?;

    for id in &candidate.replaced {
        let Some(position) = plan.unchanged.iter().position(|unchanged| unchanged == id) else {
            continue;
        };
        plan.unchanged.remove(position);
        if let Some(spec) = candidate
            .document
            .modules
            .iter()
            .find(|spec| &spec.id == id)
        {
            plan.swapped.push(spec.clone());
        }
    }
    plan.control_updates.clear();
    plan.refreshed_configs.clear();
    Ok(plan)
}

/// Refuses a plan that adds, removes, rebuilds, retunes or rewires a module
/// no edit names, or rebuilds a module no edit replaced. Either means the
/// invention changed underneath the batch (a script's edit, say) in a way
/// the batch would silently undo.
pub(super) fn guard(plan: &ReloadPlan, candidate: &EditedCandidate) -> Result<(), RpcError> {
    let connections = plan
        .added_connections
        .iter()
        .chain(&plan.removed_connections)
        .flat_map(|conn| [conn.from.as_str(), conn.to.as_str()]);
    let touched = plan
        .added
        .iter()
        .chain(&plan.swapped)
        .map(|spec| spec.id.as_str())
        .chain(plan.removed.iter().map(String::as_str))
        .chain(plan.control_updates.iter().map(|(id, _, _)| id.as_str()))
        .chain(connections);
    for id in touched {
        if !candidate.named_modules.contains(id) {
            return Err(outside_batch(format!(
                "apply_edits would change module '{id}', which no edit in the batch names"
            )));
        }
    }
    if let Some(spec) = plan
        .swapped
        .iter()
        .find(|spec| !candidate.replaced.contains(&spec.id))
    {
        return Err(outside_batch(format!(
            "apply_edits would rebuild module '{}', which no edit in the batch replaces",
            spec.id
        )));
    }
    Ok(())
}

fn outside_batch(message: String) -> RpcError {
    RpcError::new(
        RpcErrorCode::Internal,
        format!(
            "{message}; the invention changed while the batch was checked, so nothing was applied"
        ),
    )
}
