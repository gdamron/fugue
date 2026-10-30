//! What a structural edit changed in the graph, as the difference between
//! two revisions.
//!
//! A host keeps the graph's shape (module ids and types, and connections) as
//! it last announced it. When an edit lands, from any client, it compares the
//! new shape with that one and announces the difference as a
//! [`GraphChange`], so a view can show what changed: added and rewired
//! modules called out, removed ones briefly drawn where they were. The
//! difference is taken from the two shapes alone, so it is the same whichever
//! command made the edit, and nothing about the edit's author is recorded yet.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{RuntimeFullSnapshot, RuntimeRevision};
use crate::{RuntimeConnectionInfo, RuntimeState};

/// Most modules and connections one change lists. A larger change (a whole
/// new piece pasted in, say) is announced without its lists, see
/// [`GraphChange::too_large`], which keeps every announcement small.
pub const MAX_GRAPH_CHANGE_ITEMS: usize = 128;

/// A module named in a change: its id and its type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ChangedModule {
    pub id: String,
    pub module_type: String,
}

/// The graph's shape: its modules (id and type, in module order) and its
/// connections. Values and configs are not part of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphShape {
    pub modules: Vec<ChangedModule>,
    pub connections: Vec<RuntimeConnectionInfo>,
}

impl GraphShape {
    /// The shape of a snapshot's graph.
    pub fn of_snapshot(snapshot: &RuntimeFullSnapshot) -> Self {
        Self {
            modules: (snapshot.modules.iter())
                .map(|m| ChangedModule {
                    id: m.info.id.clone(),
                    module_type: m.info.module_type.clone(),
                })
                .collect(),
            connections: snapshot.connections.clone(),
        }
    }

    /// The shape of a runtime state's graph.
    pub fn of_state(state: &RuntimeState) -> Self {
        Self {
            modules: (state.modules.values())
                .map(|m| ChangedModule {
                    id: m.id.clone(),
                    module_type: m.module_type.clone(),
                })
                .collect(),
            connections: state.connections.clone(),
        }
    }
}

/// What one structural edit changed, between revision `from` and revision
/// `to`. Each list keeps graph order: modules added and replaced in the new
/// graph's module order, removed ones in the old graph's, and connections in
/// the order each graph lists them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct GraphChange {
    /// The revision the graph had before the change.
    pub from: RuntimeRevision,
    /// The revision the change produced.
    pub to: RuntimeRevision,
    /// Modules new to the graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<ChangedModule>,
    /// Modules gone from the graph, with the type they had.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<ChangedModule>,
    /// Modules that kept their id but changed type, with their new type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<ChangedModule>,
    /// Ids of modules that stayed (same id and type) but gained or lost a
    /// connection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewired: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections_added: Vec<RuntimeConnectionInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections_removed: Vec<RuntimeConnectionInfo>,
    /// How many modules and connections changed in all: the length of every
    /// list above together, even when they are left empty.
    pub change_count: usize,
    /// True when more than [`MAX_GRAPH_CHANGE_ITEMS`] changed: every list is
    /// then empty, and a view shows the new graph without calling anything
    /// out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub too_large: bool,
}

impl GraphChange {
    /// The difference from `before` to `after`, or none when the graph's
    /// shape is the same (a value change, say). A module that keeps its id
    /// but changes type is replaced, not removed and added, and is not also
    /// listed as rewired.
    pub fn between(
        before: &GraphShape,
        after: &GraphShape,
        from: RuntimeRevision,
        to: RuntimeRevision,
    ) -> Option<Self> {
        let old_types: HashMap<&str, &str> = (before.modules.iter())
            .map(|m| (m.id.as_str(), m.module_type.as_str()))
            .collect();
        let new_ids: HashSet<&str> = after.modules.iter().map(|m| m.id.as_str()).collect();
        let mut added = Vec::new();
        let mut replaced = Vec::new();
        for m in &after.modules {
            match old_types.get(m.id.as_str()) {
                None => added.push(m.clone()),
                Some(old) if *old != m.module_type => replaced.push(m.clone()),
                Some(_) => {}
            }
        }
        let removed: Vec<ChangedModule> = (before.modules.iter())
            .filter(|m| !new_ids.contains(m.id.as_str()))
            .cloned()
            .collect();
        let connections_added = missing_from(&after.connections, &before.connections);
        let connections_removed = missing_from(&before.connections, &after.connections);

        // Kept modules at either end of a connection that came or went.
        let not_kept: HashSet<&str> = (added.iter().chain(&removed).chain(&replaced))
            .map(|m| m.id.as_str())
            .collect();
        let touched: HashSet<&str> = (connections_added.iter().chain(&connections_removed))
            .flat_map(|c| [c.from.as_str(), c.to.as_str()])
            .filter(|id| !not_kept.contains(id) && new_ids.contains(id))
            .collect();
        let rewired: Vec<String> = (after.modules.iter())
            .filter(|m| touched.contains(m.id.as_str()))
            .map(|m| m.id.clone())
            .collect();

        let change_count = added.len()
            + removed.len()
            + replaced.len()
            + rewired.len()
            + connections_added.len()
            + connections_removed.len();
        if change_count == 0 {
            return None;
        }
        let mut change = Self {
            from,
            to,
            added,
            removed,
            replaced,
            rewired,
            connections_added,
            connections_removed,
            change_count,
            too_large: false,
        };
        if change_count > MAX_GRAPH_CHANGE_ITEMS {
            change.added.clear();
            change.removed.clear();
            change.replaced.clear();
            change.rewired.clear();
            change.connections_added.clear();
            change.connections_removed.clear();
            change.too_large = true;
        }
        Some(change)
    }
}

/// The connections in `these` that `those` lacks, in `these`' order.
fn missing_from(
    these: &[RuntimeConnectionInfo],
    those: &[RuntimeConnectionInfo],
) -> Vec<RuntimeConnectionInfo> {
    let present: HashSet<&RuntimeConnectionInfo> = those.iter().collect();
    (these.iter())
        .filter(|c| !present.contains(c))
        .cloned()
        .collect()
}
