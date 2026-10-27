//! Requests timed by a condition, submitted to a live graph by module id
//! (see [`Condition`]).
//!
//! A condition applies its request on the exact sample it is met only when
//! every module whose output it watches computes before the module the
//! request targets (see `graph::conditions`). So submitting one also orders
//! each such module not yet ordered either way before the target, by an
//! ordering-only edge published right ahead of the request. A watched module
//! the target already runs into stays where it is, as does one whose order
//! would change what is heard (see `change::order`). That request applies
//! late, at its target's next segment, and says so. Orders outlive their
//! requests until the first change made once no condition waits.

use std::any::Any;

use super::change::{SurfaceMap, TopologyMirror};
use super::{LiveGraph, PendingWrite, PreparedChange, Publisher};
use crate::control_request::{Condition, Edge, Predicate, Request, RequestId, RequestValue, When};
use crate::invention::declared::DeclaredSurface;
use crate::invention::runtime::GraphCommandError;
use crate::ControlValue;

/// One thing a condition submitted by module id waits for (see
/// [`Predicate`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Part<'a> {
    /// The `count`th time output `port` of `module` crosses `level` the
    /// `edge` way: a note, a step, a beat, or a threshold (a count of one).
    Signal {
        module: &'a str,
        port: &'a str,
        edge: Edge,
        level: f32,
        count: u32,
    },
    /// Control `key` of `module` crossing `level` the `edge` way, read at
    /// segment starts.
    Control {
        module: &'a str,
        key: &'a str,
        edge: Edge,
        level: f32,
    },
    /// Another request applying.
    Applied(RequestId),
}

impl Part<'_> {
    /// The module whose output it reads as the segment computes, if any.
    /// A control is read at segment starts, whatever the order.
    fn output_of(&self) -> Option<&str> {
        match *self {
            Self::Signal { module, .. } => Some(module),
            Self::Control { .. } | Self::Applied(_) => None,
        }
    }
}

impl LiveGraph {
    /// Writes `value` to control `key` of module `module_id` on the sample
    /// every one of `parts` (one to four) has happened, counted from when
    /// the audio thread takes the request, or refuses it `Expired` once
    /// `ttl` samples have passed since now. Returns the request's id, whose
    /// outcome [`Self::take_outcomes`] reports.
    ///
    /// Refused with nothing submitted when the module, a port or a control
    /// is unknown, the value does not fit the control, a part can never be
    /// met, or [`CONDITION_CAPACITY`] requests already wait on conditions
    /// (`ControlError`, to retry once some settle).
    ///
    /// [`CONDITION_CAPACITY`]: crate::control_request::CONDITION_CAPACITY
    pub(crate) fn submit_when(
        &self,
        (module_id, key): (&str, &str),
        value: &ControlValue,
        parts: &[Part<'_>],
        ttl: Option<u64>,
    ) -> Result<RequestId, GraphCommandError> {
        let control = GraphCommandError::ControlError;
        self.reclaim();
        let mut publisher = self.lock();
        if !publisher.audio_alive() {
            return Err(GraphCommandError::AudioThreadStopped);
        }
        if self.requests.watchers_full() {
            let full = "Too many requests wait on a condition; try again once some apply";
            return Err(control(full.into()));
        }
        // Publisher, then surfaces, as a commit takes them.
        let surfaces = self.control_surfaces.lock().unwrap();
        let surface = surfaces.get(module_id);
        let surface = surface.ok_or_else(|| GraphCommandError::UnknownModule(module_id.into()))?;
        let declared = surface
            .as_any()
            .and_then(|any| any.downcast_ref::<DeclaredSurface>());
        let declared = declared.ok_or_else(|| {
            control(format!(
                "{module_id}'s controls cannot be timed by a condition"
            ))
        })?;
        let write = declared.resolve_write(key, value).map_err(control)?;
        let condition = condition_of(publisher.mirror(), &surfaces, parts)?;
        drop(surfaces);

        // The order goes into the queue ahead of the request, so the audio
        // thread takes the request only once the order is installed: it
        // never starts reading in the order before. Should the order be
        // refused, the request still stands, and applies late.
        let mut change = self.change_on(&publisher);
        for source in parts.iter().filter_map(Part::output_of) {
            change.order(source, module_id);
        }
        let ordered = self.publish_order(&mut publisher, change.compile());

        // Resolved against the order's generation: an order moves no
        // module, so the condition's indices hold in it too.
        let mut request = Request::new(
            publisher.control_target(module_id, write.index)?,
            RequestValue::Value(write.raw),
        );
        request.when = When::Condition(condition);
        request.ttl = ttl;
        request.event = write.event;
        let (source, when) = (request.source, request.when);

        // Recorded as a declared surface records its writes (see
        // `invention::declared`): the log locked before the submission.
        let mut pending = self.pending.lock().unwrap();
        pending.settle();
        let queue_full = |_| GraphCommandError::QueueFull;
        let id = self.requests.submit(request).map_err(queue_full)?;
        if let Some((key, value)) = write.shown {
            let module_id = module_id.to_string();
            let write = PendingWrite {
                module_id,
                key,
                value,
                raw: Some(write.raw),
                beat: None,
                source,
                when,
                authored: None,
                announced: false,
                awaited: false,
                submitted: 0,
            };
            pending.submitted(id, write);
        }
        drop(pending);
        drop(publisher);
        // What the order leaves to free goes once the guard is released.
        drop(ordered);
        self.settle();
        Ok(id)
    }

    /// Publishes `prepared`, holding only orders (see `change::order`),
    /// under `publisher`: an order moves no module and routes nothing, so
    /// no surface, port or connection changes with it. Returns what is left
    /// to free once the guard is released; publishes nothing when the
    /// queue is backed up, a timed edit waits (see `publisher::timed`), or
    /// the publisher refuses, or a module was
    /// retargeted between ordering and compiling (see `change::order`).
    fn publish_order(&self, publisher: &mut Publisher, prepared: PreparedChange) -> Box<dyn Any> {
        // An order would cancel a timed edit waiting: the request applies
        // late instead.
        let waits = publisher.timed_waiting();
        if waits || prepared.is_empty() || prepared.mirror.targets != publisher.mirror().targets {
            return Box::new(prepared);
        }
        if !self.reclaimer.threaded() && publisher.audio_alive() && self.backlogged(publisher) {
            return Box::new(prepared);
        }
        // Under the publisher, as a commit records itself.
        self.pending.lock().unwrap().edited();
        match publisher.publish(prepared) {
            Ok(published) => {
                debug_assert!(published.built.is_empty(), "an order builds nothing");
                Box::new(published)
            }
            Err(refused) => Box::new(refused),
        }
    }
}

/// Resolves `parts` against the publisher's mirror and the surfaces into a
/// condition all of them meet, refusing one that can never be met.
pub(super) fn condition_of(
    mirror: &TopologyMirror,
    surfaces: &SurfaceMap,
    parts: &[Part<'_>],
) -> Result<Condition, GraphCommandError> {
    let predicates = parts
        .iter()
        .map(|part| predicate_of(mirror, surfaces, part));
    let predicates = predicates.collect::<Result<Vec<_>, _>>()?;
    let condition = Condition::all(&predicates).ok_or_else(|| {
        GraphCommandError::ControlError("A condition joins one to four parts".into())
    })?;
    condition.check().map_err(|refusal| {
        GraphCommandError::ControlError(format!("This condition can never be met: {refusal:?}"))
    })?;
    Ok(condition)
}

fn predicate_of(
    mirror: &TopologyMirror,
    surfaces: &SurfaceMap,
    part: &Part<'_>,
) -> Result<Predicate, GraphCommandError> {
    let unknown = |module: &str| GraphCommandError::UnknownModule(module.into());
    Ok(match *part {
        Part::Signal {
            module,
            port,
            edge,
            level,
            count,
        } => {
            let (index, _, watched) = mirror
                .modules
                .get_full(module)
                .ok_or_else(|| unknown(module))?;
            let outputs = &watched.ports.outputs;
            let at = outputs
                .iter()
                .position(|name| name == port)
                .ok_or_else(|| {
                    GraphCommandError::InvalidPort(format!("{module} has no output {port}"))
                })?;
            Predicate::Signal {
                module: index as u32,
                port: at as u16,
                edge,
                level,
                count,
            }
        }
        Part::Control {
            module,
            key,
            edge,
            level,
        } => {
            let index = mirror
                .modules
                .get_index_of(module)
                .ok_or_else(|| unknown(module))?;
            let declared = surfaces
                .get(module)
                .and_then(|surface| surface.declaration(key));
            let declared = declared.ok_or_else(|| {
                GraphCommandError::ControlError(format!("{module} declares no control {key}"))
            })?;
            Predicate::Control {
                module: index as u32,
                control: declared.index,
                edge,
                level,
            }
        }
        Part::Applied(id) => Predicate::Applied(id),
    })
}
