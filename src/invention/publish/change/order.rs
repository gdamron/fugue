//! Ordering-only edges for requests timed by a condition (see
//! `graph::conditions`): a module a condition watches computes before the
//! module its request targets, so the request applies on its exact sample.

use super::{GraphChange, MirrorFacts};
use crate::invention::graph::TopologyFacts;

impl GraphChange {
    /// Orders module `source` before module `target` without routing
    /// anything between them. Adds nothing, and returns false, when the
    /// order already holds, or when `target` already runs into `source`:
    /// that order would close a cycle and fold both into a feedback group,
    /// whose one-sample delays change what is heard. The request then
    /// applies late, at the target's next segment.
    pub(crate) fn order(&mut self, source: &str, target: &str) -> bool {
        if source == target || self.reaches(source, target) || self.reaches(target, source) {
            return false;
        }
        let watch = (source.to_string(), target.to_string());
        self.mirror.watches.push(watch);
        self.edits += 1;
        true
    }

    /// Drops every order [`Self::order`] added, once no condition waits on
    /// them. Not an edit: they go with whatever else the change publishes.
    pub(crate) fn forget_orders(&mut self) {
        self.mirror.watches.clear();
    }

    /// Whether module `from` computes before module `to` by what orders
    /// modules: routes, control targets and watches.
    fn reaches(&self, from: &str, to: &str) -> bool {
        let facts = MirrorFacts {
            mirror: &self.mirror,
            surfaces: &self.surfaces,
        };
        let (Some(from), Some(to)) = (facts.index_of(from), facts.index_of(to)) else {
            return false;
        };
        let mut seen = vec![false; facts.module_count()];
        let mut next = vec![from];
        while let Some(module) = next.pop() {
            if module == to {
                return true;
            }
            if std::mem::replace(&mut seen[module], true) {
                continue;
            }
            let (id, _) = self.mirror.modules.get_index(module).unwrap();
            let routed = self
                .mirror
                .edges
                .iter()
                .filter(|edge| edge.from_module == *id);
            let routed = routed.map(|edge| edge.to_module.clone());
            let downstream = routed.chain(facts.control_targets(module));
            next.extend(downstream.filter_map(|id| facts.index_of(&id)));
        }
        false
    }
}
