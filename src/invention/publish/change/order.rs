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
//! An order publishes on its own, so it is added only while a compile now
//! is the one installed: while every module writes the controls it wrote
//! when the topology was last published. A scheduler retargeted live
//! writes others, yet the installed topology still orders it by the old
//! ones (FUG-388); a compile now would order it anew.

use indexmap::IndexMap;

use super::{GraphChange, MirrorFacts};
use crate::invention::graph::{compile_topology, CompiledTopology, TopologyFacts};

/// Each module's control targets, by id (see
/// [`crate::Module::control_targets`]).
pub(crate) type Targets = IndexMap<String, Vec<String>>;

/// What a topology does to the sound beyond acyclic order: each feedback
/// group's members in the order they compute, every route with whether it
/// is delayed a sample, and the modules writing controls in the order they
/// compute.
type Heard = (
    Vec<Vec<usize>>,
    Vec<(usize, usize, usize, usize, bool)>,
    Vec<usize>,
);

impl GraphChange {
    /// Orders module `source` before module `target` without routing
    /// anything between them. Adds nothing, and returns false, when the
    /// order already holds or would change what is heard, or while a
    /// compile now would not reproduce the topology installed.
    pub(crate) fn order(&mut self, source: &str, target: &str) -> bool {
        let targets = self.targets_now();
        if source == target || !self.compiles_as_installed(&targets) {
            return false;
        }
        if self.reaches(&targets, source, target) || self.reaches(&targets, target, source) {
            return false;
        }
        let unordered = self.heard(&targets);
        let watch = (source.to_string(), target.to_string());
        self.mirror.watches.push(watch);
        if self.heard(&targets) != unordered {
            self.mirror.watches.pop();
            return false;
        }
        self.edits += 1;
        true
    }

    /// Each module's control targets as its surface reports them now, or
    /// as a schedule the change writes to it does (see
    /// `Rescheduling::targets`): what the change compiles with.
    pub(super) fn targets_now(&self) -> Targets {
        let mut scheduled = self.rescheduling.targets(&self.surfaces);
        let targets = self.mirror.modules.keys().map(|id| {
            let surface = self.surfaces.get(id);
            let targets = scheduled
                .remove(id)
                .or_else(|| surface.map(|surface| surface.control_targets()));
            (id.clone(), targets.unwrap_or_default())
        });
        targets.collect()
    }

    /// Whether every module the topology last published was compiled with
    /// still writes the controls it did then (`targets` being what they
    /// write now), so that a compile now orders them as installed.
    fn compiles_as_installed(&self, targets: &Targets) -> bool {
        let mut installed = self.mirror.targets.iter();
        installed.all(|(id, was)| targets.get(id).is_none_or(|now| now == was))
    }

    /// Drops every order [`Self::order`] added, once no condition waits on
    /// them. Not an edit: they go with whatever else the change publishes.
    pub(crate) fn forget_orders(&mut self) {
        self.mirror.watches.clear();
    }

    /// Drops each order that, with the change's other edits, would change
    /// what is heard: kept in the order they were added while, together,
    /// they leave it as it is with none. Run as the change compiles, once
    /// the mirror holds the targets it compiles with.
    pub(super) fn keep_orders_inaudible(&mut self) {
        if self.mirror.watches.is_empty() {
            return;
        }
        let watches = std::mem::take(&mut self.mirror.watches);
        let unordered = self.heard(&self.mirror.targets);
        for watch in watches {
            self.mirror.watches.push(watch);
            if self.heard(&self.mirror.targets) != unordered {
                self.mirror.watches.pop();
            }
        }
    }

    /// What the topology as edited so far, compiled with `targets`, does
    /// to the sound.
    fn heard(&self, targets: &Targets) -> Heard {
        let mirror = &self.mirror;
        let facts = MirrorFacts { mirror, targets };
        let topology = compile_topology(&facts, &mirror.edges, &mirror.sinks(), 1);
        heard_of(&topology, |module| {
            let (id, _) = mirror.modules.get_index(module).unwrap();
            targets.get(id).is_some_and(|targets| !targets.is_empty())
        })
    }

    /// Whether module `from` computes before module `to` by what orders
    /// modules: routes, control targets and watches.
    fn reaches(&self, targets: &Targets, from: &str, to: &str) -> bool {
        let mirror = &self.mirror;
        let facts = MirrorFacts { mirror, targets };
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
