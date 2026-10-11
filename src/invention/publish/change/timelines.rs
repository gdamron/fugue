//! The mirror's timeline contexts (see `crate::invention::timelines`).

use super::TopologyMirror;
use crate::invention::timelines::{self, Flow, Link};

impl TopologyMirror {
    /// Works out every module's timeline context for the topology as it
    /// stands (see `crate::invention::timelines`). Run whenever a topology
    /// is compiled for publication, so the publisher's mirror always holds
    /// the contexts of the generation it describes.
    pub(crate) fn settle_timelines(&mut self) {
        let flows: Vec<Flow> = self.modules.values().map(|m| m.flow.clone()).collect();
        let links: Vec<Link> = self
            .edges
            .iter()
            .filter_map(|edge| {
                let (from, _, source) = self.modules.get_full(&edge.from_module)?;
                let (to, _, dest) = self.modules.get_full(&edge.to_module)?;
                Some(Link {
                    from,
                    from_port: source
                        .ports
                        .outputs
                        .iter()
                        .position(|p| *p == edge.from_port)?,
                    to,
                    to_port: dest.ports.inputs.iter().position(|p| *p == edge.to_port)?,
                })
            })
            .collect();
        self.timelines = timelines::contexts(&flows, &links);
    }
}
