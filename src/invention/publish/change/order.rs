//! Ordering-only edges for requests timed by a condition (see
//! `graph::conditions`): a module a condition watches computes before the
//! module its request targets, so the request applies on its exact sample.
//!
//! An order never changes what is heard. Acyclic modules may compute in
//! any order their routes allow, but a feedback group's members, their
//! order, and which of its routes are delayed a sample are the sound
//! itself, as is the order of the modules that write other modules'
//! controls: of two writing one control, the later wins. So an order that
//! would change any of them (closing a cycle, moving where the order
//! search enters an existing loop, or reordering writers) is not added,
//! and one a later edit would make do so is dropped as that edit
//! compiles. The request then applies late, at its target's next segment.
//!
//! An order publishes on its own, so it is added only while publishing
//! the topology again would change nothing heard either: a scheduler
//! retargeted live writes other controls than the installed topology
//! orders it by (FUG-388), and a compile now would order it anew.

use indexmap::IndexMap;
use std::collections::HashMap;

use super::{GraphChange, MirrorFacts};
use crate::invention::graph::{
    compile_topology, CompiledTopology, RoutingConnection, TopologyFacts,
};
use crate::invention::runtime::ModuleInstance;
use crate::GraphModule;

/// What a topology does to the sound beyond acyclic order: each feedback
/// group's members in the order they compute, every route with whether it
/// is delayed a sample, and the modules writing controls in the order they
/// compute.
pub(crate) type Heard = (
    Vec<Vec<usize>>,
    Vec<(usize, usize, usize, usize, bool)>,
    Vec<usize>,
);

impl GraphChange {
    /// Orders module `source` before module `target` without routing
    /// anything between them. Adds nothing, and returns false, when the
    /// order already holds or would change what is heard.
    pub(crate) fn order(&mut self, source: &str, target: &str) -> bool {
        if source == target || self.reaches(source, target) || self.reaches(target, source) {
            return false;
        }
        let unordered = self.heard();
        if unordered != self.mirror.heard {
            return false;
        }
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
            scheduled: self.rescheduling.targets(&self.surfaces),
        };
        let sinks = self.mirror.sinks();
        let topology = compile_topology(&facts, &self.mirror.edges, &sinks, 1);
        self.heard_of(&topology, &facts.scheduled)
    }

    /// What `topology`, compiled from this change with the schedules it
    /// writes, `scheduled`, does to the sound.
    pub(super) fn heard_of(
        &self,
        topology: &CompiledTopology,
        scheduled: &HashMap<String, Vec<String>>,
    ) -> Heard {
        heard_of(topology, |module| {
            let (id, _) = self.mirror.modules.get_index(module).unwrap();
            // A schedule the change writes stands in for the surface's own.
            match scheduled.get(id) {
                Some(targets) => !targets.is_empty(),
                None => (self.surfaces.get(id))
                    .is_some_and(|surface| !surface.control_targets().is_empty()),
            }
        })
    }

    /// Whether module `from` computes before module `to` by what orders
    /// modules: routes, control targets and watches.
    fn reaches(&self, from: &str, to: &str) -> bool {
        let facts = MirrorFacts {
            mirror: &self.mirror,
            surfaces: &self.surfaces,
            scheduled: self.rescheduling.targets(&self.surfaces),
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

/// What the graph about to go live, `modules` routed by `edges`, does to
/// the sound.
pub(super) fn heard_live(
    modules: &IndexMap<String, ModuleInstance>,
    edges: &[RoutingConnection],
) -> Heard {
    let sinks = modules
        .iter()
        .filter(|(_, module)| matches!(module, GraphModule::Sink(_)));
    let sinks: Vec<_> = sinks.map(|(id, _)| id.clone()).collect();
    let topology = compile_topology(modules, edges, &sinks, 1);
    heard_of(&topology, |module| {
        !modules[module].module().control_targets().is_empty()
    })
}

/// What `topology` does to the sound, given which modules write controls.
fn heard_of(topology: &CompiledTopology, writes: impl Fn(usize) -> bool) -> Heard {
    // In the order they run: group by group, as the graph processes.
    let groups = topology.process_groups.iter();
    let run = groups.flat_map(|group| group.members.iter().copied());
    let writers = run.filter(|module| writes(*module)).collect();
    let feedback = topology
        .process_groups
        .iter()
        .filter(|group| group.feedback);
    let mut loops: Vec<_> = feedback.map(|group| group.members.clone()).collect();
    loops.sort_unstable();
    let mut routes = Vec::new();
    for (to, into) in topology.compiled_routes.iter().enumerate() {
        for route in into {
            let from = (route.from_module, route.from_port);
            routes.push((from.0, from.1, to, route.to_port, route.delayed));
        }
    }
    routes.sort_unstable();
    (loops, routes, writers)
}
