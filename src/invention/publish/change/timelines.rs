//! The mirror's timeline contexts (see `crate::invention::timelines`).

use super::TopologyMirror;
use crate::invention::runtime::GraphCommandError;
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

    /// The index of the invention's first clock, the one a timed edit
    /// batch or reload counts beats on unless it names one.
    pub(crate) fn first_clock(&self) -> Result<u32, GraphCommandError> {
        let mut modules = self.modules.values();
        let clock = modules.position(|module| matches!(module.flow, Flow::Clock));
        let none = "The invention has no clock to count beats on: time this edit in samples";
        // Module counts are far below u32::MAX (`BeatTime` keeps it narrow).
        clock
            .map(|index| index as u32)
            .ok_or_else(|| GraphCommandError::ControlError(none.into()))
    }

    /// Resolves the clock a request timed in beats on `module_id` counts on,
    /// for [`BeatTime::clock`](crate::control_request::BeatTime): `clock`
    /// by id when the request names one, otherwise the module's timeline
    /// context (see `crate::invention::timelines`). The request keeps this
    /// clock whatever later changes do to the context, and is refused
    /// `TimelineGone` if the clock goes.
    ///
    /// Fails with [`GraphCommandError::UnknownModule`], or with
    /// [`GraphCommandError::ControlError`] when the named module is not a
    /// clock, or an unnamed clock cannot be resolved: the invention has no
    /// clock a request can name, or the module is driven by a clock inside a
    /// development, which a request cannot name yet.
    pub(crate) fn beat_clock(
        &self,
        module_id: &str,
        clock: Option<&str>,
    ) -> Result<u32, GraphCommandError> {
        let modules = &self.modules;
        let index_of = |id: &str| {
            modules
                .get_index_of(id)
                .ok_or_else(|| GraphCommandError::UnknownModule(id.to_string()))
        };
        let module = index_of(module_id)?;
        let clock = match clock {
            Some(id) => match index_of(id)? {
                clock if matches!(modules[clock].flow, Flow::Clock) => clock,
                _ => {
                    return Err(GraphCommandError::ControlError(format!(
                        "module '{id}' is not a clock"
                    )));
                }
            },
            None => match self.timelines.get(module) {
                Some(Context::Connected { clock, .. } | Context::Fallback { clock }) => *clock,
                Some(Context::Enclosed { development }) => {
                    let development = modules.get_index(*development).map_or("?", |(id, _)| id);
                    return Err(GraphCommandError::ControlError(format!(
                        "module '{module_id}' counts on a clock inside development \
                         '{development}', which a request cannot name yet; name a clock"
                    )));
                }
                Some(Context::None) | None => {
                    return Err(GraphCommandError::ControlError(format!(
                        "module '{module_id}' has no timeline: the invention has no clock \
                         a request can name"
                    )));
                }
            },
        };
        // Module counts are far below u32::MAX (`BeatTime` keeps it narrow).
        Ok(clock as u32)
    }
}
