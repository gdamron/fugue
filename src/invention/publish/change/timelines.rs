//! The mirror's timeline contexts (see `crate::invention::timelines`).

use super::TopologyMirror;
use crate::invention::timelines::{self, Context, Flow, Link};
use crate::TimelineContext;

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

    /// The timeline context of module `id`, by clock id: `None` when the
    /// module is unknown or the graph has no clock.
    pub(crate) fn timeline_of(&self, id: &str) -> Option<TimelineContext> {
        let index = self.modules.get_index_of(id)?;
        let name = |index: usize| self.modules.get_index(index).map(|(id, _)| id.clone());
        Some(match *self.timelines.get(index)? {
            Context::Connected { clock, distance } => TimelineContext::Connected {
                clock: name(clock)?,
                distance,
            },
            Context::Enclosed { development } => TimelineContext::Enclosed {
                development: name(development)?,
            },
            Context::Fallback { clock } => TimelineContext::Fallback {
                clock: name(clock)?,
            },
            Context::None => return None,
        })
    }
}
