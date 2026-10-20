//! Ordering-only edges for requests timed by a condition (see
//! `graph::conditions`): a module a condition watches computes before the
//! module its request targets, so the request applies on its exact sample.
//!
//! An order never changes what is heard. Acyclic modules may compute in
//! any order their routes allow, but a feedback group's members, their
//! order, and which of its routes are delayed a sample are the sound
//! itself. So an order that would change any of them (closing a cycle, or
//! moving where the order search enters an existing loop) is not added,
//! and one a later edit would make do so is dropped as that edit
//! compiles. The request then applies late, at its target's next segment.

use super::{GraphChange, MirrorFacts};
use crate::invention::graph::{compile_topology, TopologyFacts};

/// What a topology does to the sound beyond acyclic order: each feedback
/// group's members in the order they compute, and every route with whether
/// it is delayed a sample.
type Heard = (Vec<Vec<usize>>, Vec<(usize, usize, usize, usize, bool)>);

impl GraphChange {
    /// Orders module `source` before module `target` without routing
    /// anything between them. Adds nothing, and returns false, when the
    /// order already holds or would change what is heard.
    pub(crate) fn order(&mut self, source: &str, target: &str) -> bool {
        if source == target || self.reaches(source, target) || self.reaches(target, source) {
            return false;
        }
        let unordered = self.heard();
        let watch = (source.to_string(), target.to_string());
        self.mirror.watches.push(watch);
        if self.heard() != unordered {
            self.mirror.watches.pop();
            return false;
        }
        self.edits += 1;
        true
    }

    /// Drops every order [`Self::order`] added, once no condition waits on
    /// them. Not an edit: they go with whatever else the change publishes.
    pub(crate) fn forget_orders(&mut self) {
        self.mirror.watches.clear();
    }

    /// Drops each order that, with the change's other edits, would change
    /// what is heard: kept in the order they were added while, together,
    /// they leave it as it is with none.
    pub(super) fn keep_orders_inaudible(&mut self) {
        if self.mirror.watches.is_empty() {
            return;
        }
        let watches = std::mem::take(&mut self.mirror.watches);
        let unordered = self.heard();
        for watch in watches {
            self.mirror.watches.push(watch);
            if self.heard() != unordered {
                self.mirror.watches.pop();
            }
        }
    }

    /// What the topology as edited so far does to the sound.
    fn heard(&self) -> Heard {
        let facts = MirrorFacts {
            mirror: &self.mirror,
            surfaces: &self.surfaces,
        };
        let sinks = self.mirror.sinks();
        let topology = compile_topology(&facts, &self.mirror.edges, &sinks, 1);
        let mut loops: Vec<_> = topology
            .process_groups
            .into_iter()
            .filter(|group| group.feedback)
            .map(|group| group.members)
            .collect();
        loops.sort_unstable();
        let mut routes = Vec::new();
        for (to, into) in topology.compiled_routes.into_iter().enumerate() {
            for route in into {
                let from = (route.from_module, route.from_port);
                routes.push((from.0, from.1, to, route.to_port, route.delayed));
            }
        }
        routes.sort_unstable();
        (loops, routes)
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
