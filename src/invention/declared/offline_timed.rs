//! A timed write to a declared control of an offline render: timed as a
//! live graph's is (see `publish::timed_write`), and submitted to the
//! render's own request queue (see `graph::offline_timed`), which the
//! render's drain takes and applies where a live graph's drain would.

use super::deliver::{QUEUE_FULL, SCHEDULE_UNTIMED, UNTIMED};
use super::options::{Queued, Write, WriteOptions};
use super::DeclaredSurface;
use crate::control_request::{ControlIndex, ControlTarget, Intent, Request, RequestValue, When};
use crate::invention::graph::{OfflineWrite, SignalGraph};
use crate::invention::publish::timed_write::{self, WriteTime};
use crate::invention::publish::TopologyMirror;

/// Why a write timed after another request is refused offline.
pub(crate) const AFTER_UNTIMED: &str =
    "This render cannot time a write after another request (`after_request`) yet; \
     nothing was written";

impl DeclaredSurface {
    /// Submits a write to control `index` of module `module_id` in an
    /// offline render's `graph`, timed as `options` say, as the live route
    /// submits one: a schedule's write cannot be timed, and room for it to
    /// wait is checked first. `(event, reserved)` are the control's. The
    /// write waits in the render's queue until a render takes it.
    pub(super) fn deliver_offline_timed(
        &self,
        graph: &mut SignalGraph,
        module_id: &str,
        (index, write): (ControlIndex, Write),
        options: &WriteOptions,
        (event, reserved): (bool, bool),
    ) -> Result<Queued, String> {
        let time = options.time.ok_or(UNTIMED)?;
        let (value, shown) = match write {
            Write::Value(value) => (RequestValue::Value(value), None),
            // A schedule's write travels in a recompile, which cannot wait.
            Write::Payload(_, _, Some(_)) => return Err(SCHEDULE_UNTIMED.into()),
            Write::Payload(payload, shown, None) => (RequestValue::Payload(payload), Some(shown)),
        };
        let module_idx = (graph.modules.get_index_of(module_id))
            .ok_or("This module has been removed or replaced")?;
        // Only a beat reads the graph, for its clock.
        let timed = match time {
            WriteTime::Beat { .. } => {
                let mirror = TopologyMirror::of(&graph.modules, &graph.edges);
                timed_write::placed(time, module_id, &mirror)
            }
            _ => timed_write::placed(time, module_id, &TopologyMirror::default()),
        };
        let timed = timed.ok_or(AFTER_UNTIMED)??;
        let target = ControlTarget {
            generation: graph.request_generation(),
            module_idx,
            control: index,
        };
        let offline = graph.offline.as_mut().ok_or(UNTIMED)?;
        offline.settle();
        timed_write::check_room(&timed, offline.waiting(), reserved)?;
        let mut request = Request::new(target, value);
        request.event = event;
        request.reserved = reserved;
        request.intent = match options.authored {
            Some(_) => Intent::Author,
            None => Intent::Perform,
        };
        request.source = options.source;
        request.priority = options.priority;
        request.ttl = options.ttl;
        // Counted from now as the sender would count it, so the reply
        // knows the sample it is due on.
        request.when = match timed.when {
            When::AfterSamples(samples) => {
                When::AtSample(offline.sender.rendered().saturating_add(samples))
            }
            when => when,
        };
        let when = request.when;
        if !offline.sender.has_room() {
            return Err(QUEUE_FULL.into());
        }
        let id = (offline.sender.submit(request)).map_err(|_| QUEUE_FULL.to_string())?;
        if let Some(shown) = shown {
            self.show(index, shown, None);
        }
        let key = self.table.key(index).unwrap_or_default();
        let module_id = module_id.to_string();
        let beat = timed.beat;
        let authored = options.authored.map(|authoring| authoring.value.clone());
        let write = OfflineWrite {
            module_id,
            key,
            when,
            beat,
            authored,
            generation: target.generation,
            announced: true,
        };
        offline.submitted(id, write);
        Ok(Queued {
            id: Some(id),
            ..Queued::default()
        })
    }
}
