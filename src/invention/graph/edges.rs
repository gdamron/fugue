//! Tables played on rising edges on the audio thread: what the graph lets
//! the drain's edge cursors read (see
//! [`crate::control_request::EdgeCursors`]), and which modules they split.
//!
//! A table's module counts its clock input's edges as it computes; the
//! drain counts the same inputs as each target is about to compute, so an
//! entry lands on its edge's sample, and the module's `step` output and
//! its count agree.

use super::SignalGraph;
use crate::control_request::{BeatHost, BeatTable, EdgeHost, Ports, Watches};

impl EdgeHost for SignalGraph {
    fn segment(&self) -> (u64, usize) {
        (self.segment.start, self.segment.frames)
    }

    fn input(&self, module: usize, port: Option<usize>, frame: usize) -> f32 {
        let (Some(port), Some(routes)) = (port, self.compiled_routes.get(module)) else {
            return 0.0;
        };
        let mut routes = routes
            .iter()
            .filter(|route| route.to_port == port)
            .peekable();
        if routes.peek().is_none() {
            // Unconnected, it holds what was written to it, as its module
            // reads it.
            let table = self.table(module);
            return table.map_or(0.0, |table| table.held_input(port));
        }
        // Summed in route order from 0, as `process_span` sums them.
        let mut sum = 0.0;
        for route in routes {
            let base = route.from_port * self.block_capacity;
            sum += self.out_bufs[route.from_module][base + frame];
        }
        sum
    }

    fn splits(&self, module: usize, ports: Ports, target: usize) -> bool {
        let Some(&group) = self.group_of.get(target) else {
            return false;
        };
        if self.process_groups[group].feedback || self.sink_indices.contains(&target) {
            return false;
        }
        let Some(routes) = self.compiled_routes.get(module) else {
            return false;
        };
        routes
            .iter()
            .filter(|route| route.to_port == ports.clock || Some(route.to_port) == ports.reset)
            .all(|route| self.group_of[route.from_module] < group)
    }

    fn module_index(&self, id: &str) -> Option<usize> {
        self.modules.get_index_of(id)
    }
}

impl SignalGraph {
    /// Whether the drain plays a table on edges: then every segment looks
    /// for them as its targets compute.
    pub(crate) fn edges_playing(&self) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|drain| !drain.watches.edges.is_empty())
    }

    /// The inputs table `table` of the module at `module` counts edges on,
    /// when it can be played on them: its module computes outside any
    /// feedback group, after every module feeding them, with something
    /// connected to its clock input.
    pub(super) fn edge_ports(&self, module: usize, table: &dyn BeatTable) -> Option<Ports> {
        let (_, instance) = self.modules.get_index(module)?;
        let group = *self.group_of.get(module)?;
        if self.process_groups.get(group)?.feedback {
            return None;
        }
        let inputs = instance.module();
        let clock = inputs.input_port_index(table.clock_input())?;
        let reset = table
            .reset_input()
            .and_then(|reset| inputs.input_port_index(reset));
        let routes = self.compiled_routes.get(module)?;
        routes
            .iter()
            .any(|route| route.to_port == clock)
            .then_some(Ports { clock, reset })
    }

    /// Counts the segment's edges at its end, playing late what falls due
    /// on targets not split on them.
    pub(super) fn commit_edges(&mut self) {
        self.with_drain(|graph, drain| {
            let Watches { edges, ramps, .. } = &mut drain.watches;
            edges.commit(graph, ramps);
        });
    }
}
