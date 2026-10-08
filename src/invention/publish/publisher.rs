//! The single control-thread publisher of a live graph's topology.

use indexmap::IndexMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use super::change::{BuiltModule, PreparedChange, TopologyMirror};
use crate::audio_thread::debug_assert_control_thread;
use crate::control_request::{
    outcome_channel, OutcomeReceiver,request_channel, ControlIndex, ControlTarget, RequestSender};
use crate::invention::graph::{
    AudioLink, InputWrite, Mailbox, Publication, RequestDrain, SignalGraph, MAX_INPUT_PORT_NAME,
};
use crate::invention::runtime::GraphCommandError;
use crate::payload::{self, RetireQueue, Retirer, MAX_RETIRES_PER_REQUEST};
use crate::spsc::{Consumer, Producer, Ring};

/// Input writes that may wait for the audio thread before a write is
/// refused with [`GraphCommandError::QueueFull`].
pub(crate) const INPUT_QUEUE_CAPACITY: usize = 256;

/// Control requests that may wait in the queue before a submission is
/// refused with `QueueFull`. The audio thread pops the queue at every block
/// start while the pending store has room, and all of it in a block that
/// installs a publication.
pub(crate) const REQUEST_QUEUE_CAPACITY: usize = 256;

/// Popped requests that may wait for their sample (or for their
/// publication to install). Twice the queue, so a full queue fits in a
/// store already half full of timed requests. While the store is full the
/// audio thread leaves requests in the queue (back-pressure), except in a
/// block that installs a publication: that one must pop every request
/// resolved against an older generation, while it can still map them, so
/// what does not fit is refused (`Refusal::PendingFull`). See
/// `graph::requests`.
pub(crate) const PENDING_REQUEST_CAPACITY: usize = 512;

/// Request outcomes the audio thread may queue before a control thread
/// receives them; beyond that they are counted dropped. Enough for every
/// request one block can settle: a full store and a full queue.
pub(crate) const OUTCOME_QUEUE_CAPACITY: usize = PENDING_REQUEST_CAPACITY + REQUEST_QUEUE_CAPACITY;

/// Payload retirements the request drain's retirer can hold while the
/// reclaimer is behind: enough for every pending request and a full queue,
/// so pending requests' reservations can never use it all and room runs out
/// only while retirements wait for the reclaimer (see `graph::requests`).
pub(crate) const PAYLOAD_RETIRE_HOLD: usize =
    (PENDING_REQUEST_CAPACITY + REQUEST_QUEUE_CAPACITY) * MAX_RETIRES_PER_REQUEST;

/// Retired publications the audio thread may hand back before the control
/// thread frees them. The audio thread takes at most one publication per
/// block and the reclaimer drains the channel on a short cadence and before
/// every change, so this is rarely more than one deep; beyond it the audio
/// thread holds one more retirement and stops taking publications until
/// there is room.
pub(crate) const RETIRE_CAPACITY: usize = 8;

/// Retired publications on their way to the control thread to be freed.
pub(crate) type RetireRing = Ring<Box<Publication>>;

/// Owns the authoritative mirror of a live graph and is the only producer of
/// its structural changes. Lives behind a control-thread mutex, so prepared
/// changes publish one at a time, in order.
pub(crate) struct Publisher {
    /// The topology as of the latest publication.
    mirror: TopologyMirror,
    /// Publications made so far; a prepared change records the generation
    /// it was prepared against.
    generation: u64,
    /// Whether an input write tagged with the current generation was
    /// queued: only then does folding that generation keep a remap from it.
    written: bool,
    block_size: usize,
    publications: Arc<Mailbox<Publication>>,
    /// Queues direct input writes for the next block. Only the publisher
    /// pushes, under its lock, so writes and publications stay in order.
    inputs: Producer<InputWrite>,
    /// Publications the audio thread has installed (observed by tests).
    #[cfg_attr(not(test), allow(dead_code))]
    applied: Arc<AtomicU64>,
}

impl Publisher {
    /// Links a graph that is about to go live to a new publisher, mirroring
    /// its current modules and edges. Also returns the control side's ends
    /// of the link's channels, which need no publisher lock.
    pub(crate) fn link(graph: &mut SignalGraph) -> (Self, LinkEnds) {
        let publications = Arc::new(Mailbox::new());
        let inputs = Ring::with_capacity(INPUT_QUEUE_CAPACITY);
        let retired = RetireRing::with_capacity(RETIRE_CAPACITY);
        let (requests, request_rx) =
            request_channel(REQUEST_QUEUE_CAPACITY, Arc::clone(&graph.transport));
        let payloads = RetireQueue::with_capacity(payload::RETIRE_CAPACITY);
        let (outcome_tx, outcomes) = outcome_channel(OUTCOME_QUEUE_CAPACITY);
        graph.requests = Some(RequestDrain::new(
            request_rx,
            REQUEST_QUEUE_CAPACITY,
            PENDING_REQUEST_CAPACITY,
            Retirer::new(Arc::clone(&payloads), PAYLOAD_RETIRE_HOLD),
            outcome_tx,
        ));
        let applied = Arc::new(AtomicU64::new(0));
        graph.link = Some(AudioLink::new(
            publications.clone(),
            Consumer::claim(Arc::clone(&inputs)),
            INPUT_QUEUE_CAPACITY,
            Producer::claim(Arc::clone(&retired)),
            applied.clone(),
        ));
        let publisher = Self {
            mirror: TopologyMirror::of(&graph.modules, &graph.edges),
            generation: 0,
            written: false,
            block_size: graph.block_size,
            publications,
            inputs: Producer::claim(inputs),
            applied,
        };
        let ends = LinkEnds {
            requests,
            retired,
            payloads,
            outcomes,
        };
        (publisher, ends)
    }

    /// The topology as of the latest publication.
    pub(crate) fn mirror(&self) -> &TopologyMirror {
        &self.mirror
    }

    /// Publications made so far.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Publications the audio thread has installed so far.
    #[cfg(test)]
    pub(crate) fn applied(&self) -> u64 {
        self.applied.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resolves a write to `port` of `module_id` against the mirror, the
    /// audio graph's module order once the current generation is installed,
    /// into a record the audio thread applies without allocating. Fails
    /// with [`GraphCommandError::UnknownModule`] or
    /// [`GraphCommandError::InvalidPort`] (a port name longer than
    /// [`MAX_INPUT_PORT_NAME`] included).
    pub(crate) fn input_write(
        &self,
        module_id: &str,
        port: &str,
        value: f32,
    ) -> Result<InputWrite, GraphCommandError> {
        let (module_idx, _, module) = self
            .mirror
            .modules
            .get_full(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        let inputs = &module.ports.inputs;
        let port_idx = inputs.iter().position(|p| p == port).ok_or_else(|| {
            GraphCommandError::InvalidPort(format!(
                "module '{module_id}' does not have input port '{port}' (available: {inputs:?})"
            ))
        })?;
        if port.len() > MAX_INPUT_PORT_NAME {
            return Err(GraphCommandError::InvalidPort(format!(
                "input port name '{port}' is longer than {MAX_INPUT_PORT_NAME} bytes"
            )));
        }
        Ok(InputWrite {
            generation: self.generation,
            module_idx,
            port_idx,
            value,
        })
    }

    /// Resolves `control` of `module_id` against the mirror, the audio
    /// graph's module order once the current generation is installed, into
    /// the target a request carries. Fails with
    /// [`GraphCommandError::UnknownModule`]; whether the module has the
    /// control is checked by its declared control table.
    ///
    /// The caller submits the request before releasing the publisher and
    /// then calls [`Self::note_written`], exactly as for an input write: so
    /// the request is in the queue before any later publication, and a fold
    /// of this generation keeps the remap it needs (see
    /// `graph::requests`).
    // The front doors that submit requests arrive with FUG-310's controls.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn control_target(
        &self,
        module_id: &str,
        control: ControlIndex,
    ) -> Result<ControlTarget, GraphCommandError> {
        debug_assert_control_thread("Publisher::control_target");
        let module_idx = self
            .mirror
            .modules
            .get_index_of(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        Ok(ControlTarget {
            generation: self.generation,
            module_idx,
            control,
        })
    }

    /// Records that a write resolved by [`Self::input_write`], or a request
    /// resolved by [`Self::control_target`], was queued.
    pub(crate) fn note_written(&mut self) {
        self.written = true;
    }

    /// Queues a write resolved by [`Self::input_write`] for the next block,
    /// noting it written. Fails with [`GraphCommandError::QueueFull`] when
    /// the audio thread has not drained earlier writes, and with
    /// [`GraphCommandError::AudioThreadStopped`] when it is gone.
    pub(crate) fn queue_input(&mut self, write: InputWrite) -> Result<(), GraphCommandError> {
        if !self.audio_alive() {
            return Err(GraphCommandError::AudioThreadStopped);
        }
        self.inputs
            .push(write)
            .map_err(|_| GraphCommandError::QueueFull)?;
        self.note_written();
        Ok(())
    }

    /// Generations folded into the untaken publication that keep a remap
    /// for queued writes, if one is waiting.
    #[cfg(test)]
    pub(crate) fn pending_absorbed(&self) -> Option<Vec<u64>> {
        let pending = self.publications.take()?;
        let generations = pending.absorbed.iter().map(|a| a.generation).collect();
        drop(self.publications.put(pending));
        Some(generations)
    }

    /// Block size publications are compiled for.
    pub(crate) fn block_size(&self) -> usize {
        self.block_size
    }

    /// Whether the audio side of the link still exists.
    pub(crate) fn audio_alive(&self) -> bool {
        Arc::strong_count(&self.publications) > 1
    }

    /// Publishes a prepared change and returns the mirror it replaced.
    ///
    /// A change prepared before another publication is refused with
    /// [`GraphCommandError::TopologyMoved`]: it was validated against a
    /// topology that no longer exists, so it is never replayed onto the
    /// current one. A publication the audio thread has not taken yet is
    /// folded into this one, so a stalled stream holds at most one. Either
    /// error returns before the mailbox is touched, with nothing published.
    ///
    /// Nothing the change built is dropped here: a refused change comes back
    /// in [`Refused`], and a superseded publication in
    /// [`Published::superseded`], so the caller can drop them (a sink
    /// finalizing a file, say) after releasing the publisher.
    // A refusal is rare and on the control thread; handing the change back
    // whole is the point, so it is not boxed.
    #[allow(clippy::result_large_err)]
    pub(crate) fn publish(&mut self, prepared: PreparedChange) -> Result<Published, Refused> {
        if prepared.base_generation != self.generation {
            return Err(Refused {
                error: GraphCommandError::TopologyMoved,
                change: prepared,
            });
        }
        if !self.audio_alive() {
            return Err(Refused {
                error: GraphCommandError::AudioThreadStopped,
                change: prepared,
            });
        }
        let PreparedChange {
            mirror,
            built,
            publication,
            ..
        } = prepared;
        let Some(mut publication) = publication else {
            // An empty change publishes nothing.
            return Ok(Published {
                previous: self.mirror.clone(),
                built,
                superseded: None,
            });
        };
        // The mirror is the order the audio thread will be running when
        // it installs this, unless an untaken publication is folded in,
        // whose remap then composes in front of this one.
        publication.map_survivors(self.mirror.modules.keys().map(String::as_str));
        publication.generation = self.generation + 1;
        let superseded = self
            .publications
            .take()
            .map(|pending| publication.absorb(pending, self.written));
        // Only this publisher puts, under its lock, so the slot is empty.
        // Input writes are queued under the same lock, so every write tagged
        // with the previous generation is in the ring before this put,
        // whose release the audio thread's take acquires.
        drop(self.publications.put(publication));
        self.generation += 1;
        self.written = false;
        let previous = std::mem::replace(&mut self.mirror, mirror);
        Ok(Published {
            previous,
            built,
            superseded,
        })
    }
}

/// A change the publisher refused, with nothing published, handed back so
/// the caller drops it after releasing the publisher.
pub(crate) struct Refused {
    pub(crate) error: GraphCommandError,
    pub(crate) change: PreparedChange,
}

impl std::fmt::Debug for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Refused").field(&self.error).finish()
    }
}

/// The control side's ends of a link's queues, which need no publisher
/// lock. (Input writes are queued by the publisher itself.)
pub(crate) struct LinkEnds {
    /// Submits control requests. Submit only under the publisher (see
    /// [`Publisher::control_target`]).
    pub(crate) requests: RequestSender,
    /// Retired publications, to free off the audio thread.
    pub(crate) retired: Arc<RetireRing>,
    /// Retired request payloads, to free off the audio thread.
    pub(crate) payloads: Arc<RetireQueue>,
    /// What became of each submitted request.
    pub(crate) outcomes: OutcomeReceiver,
}

/// What a publication replaced, for committing the runtime's other mirrors.
pub(crate) struct Published {
    /// The mirror before this publication.
    pub(crate) previous: TopologyMirror,
    /// The modules the change built (instances moved to the audio thread).
    pub(crate) built: IndexMap<String, BuiltModule>,
    /// A publication the audio thread never took, folded into this one;
    /// what remains of it is the caller's to drop off the publisher lock.
    pub(crate) superseded: Option<Box<Publication>>,
}
