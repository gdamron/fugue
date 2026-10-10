//! The single control-thread publisher of a live graph's topology.

use indexmap::IndexMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::change::{BuiltModule, PreparedChange, TopologyMirror};
use crate::audio_thread::debug_assert_control_thread;
use crate::control_request::{
    outcome_channel, request_channel, ControlIndex, ControlTarget, OutcomeReceiver, QueueFull,
    Request, RequestSender, RequestValue,
};
use crate::invention::graph::{
    AudioLink, InputWrite, Publication, RequestDrain, SignalGraph, MAX_INPUT_PORT_NAME,
};
use crate::invention::runtime::GraphCommandError;
use crate::payload::{self, Payload, RetireQueue, Retirer, MAX_RETIRES_PER_REQUEST};
use crate::spsc::{Consumer, Producer, Ring};

/// Input writes that may wait for the audio thread before a write is
/// refused with [`GraphCommandError::QueueFull`].
pub(crate) const INPUT_QUEUE_CAPACITY: usize = 256;

/// Control requests that may wait in the queue, counting any edits queued
/// among them, before one is refused with `QueueFull`. The audio thread
/// pops the queue, in order, at every block start while the pending store
/// has room and the link can retire the publication an edit replaces.
pub(crate) const REQUEST_QUEUE_CAPACITY: usize = 256;

/// Queue slots beyond [`REQUEST_QUEUE_CAPACITY`] that only edits may take,
/// so a flood of control requests never shuts out a reload or an edit.
/// As many again, so the queue is a power of two; edits fill it only while
/// installs are deferred. Each queued edit keeps what its change built
/// (sample buffers, say) alive until it installs and retires, so an audio
/// side that is alive but not rendering holds up to this many before
/// publishing is refused with `QueueFull`.
pub(crate) const EDIT_RESERVE: usize = REQUEST_QUEUE_CAPACITY;

/// The request queue's size: control requests and the edits' reserve.
const QUEUE_SLOTS: usize = REQUEST_QUEUE_CAPACITY + EDIT_RESERVE;

// The queue rounds its size up to a power of two, and the reserve and the
// drain's pop budget both assume it holds exactly these slots.
const _: () = assert!(QUEUE_SLOTS.is_power_of_two());

/// Popped requests that may wait for their sample. Twice the queue, so a
/// full queue fits in a store already half full of timed requests. While
/// the store is full the audio thread leaves requests, and the edits behind
/// them, in the queue (back-pressure). See `graph::requests`.
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
/// thread frees them. The audio thread installs the edits queued since its
/// last block and the reclaimer drains the channel on a short cadence and
/// before every change, so this is rarely more than one deep; beyond it the
/// audio thread holds one more retirement and installs no further edit
/// until there is room.
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
    block_size: usize,
    /// Submits each publication as an edit request, into the queue control
    /// requests share, so the audio thread meets them in one order. It may
    /// use the slots other senders leave free ([`EDIT_RESERVE`]).
    requests: RequestSender,
    /// The newest generation queued as an edit, for the audio thread: an
    /// edit waiting there is never starved by a full pending store.
    published: Arc<AtomicU64>,
    /// Queues direct input writes for the next block. Only the publisher
    /// pushes, under its lock, so writes and publications stay in order.
    inputs: Producer<InputWrite>,
    /// Publications the audio thread has installed. The audio side holds
    /// the other reference, so it also tells whether that side still exists.
    applied: Arc<AtomicU64>,
    /// Set by [`Self::close`] as the audio side's request drain goes.
    closed: bool,
}

impl Publisher {
    /// Links a graph that is about to go live to a new publisher, mirroring
    /// its current modules and edges. Also returns the control side's ends
    /// of the link's channels, which need no publisher lock.
    pub(crate) fn link(graph: &mut SignalGraph) -> (Self, LinkEnds) {
        let inputs = Ring::with_capacity(INPUT_QUEUE_CAPACITY);
        let retired = RetireRing::with_capacity(RETIRE_CAPACITY);
        let (requests, request_rx) = request_channel(QUEUE_SLOTS, Arc::clone(&graph.transport));
        let payloads = RetireQueue::with_capacity(payload::RETIRE_CAPACITY);
        let (outcome_tx, outcomes) = outcome_channel(OUTCOME_QUEUE_CAPACITY);
        let published = Arc::new(AtomicU64::new(0));
        graph.requests = Some(RequestDrain::new(
            request_rx,
            REQUEST_QUEUE_CAPACITY,
            PENDING_REQUEST_CAPACITY,
            Retirer::new(Arc::clone(&payloads), PAYLOAD_RETIRE_HOLD),
            outcome_tx,
            Arc::clone(&published),
        ));
        let applied = Arc::new(AtomicU64::new(0));
        graph.link = Some(AudioLink::new(
            Consumer::claim(Arc::clone(&inputs)),
            INPUT_QUEUE_CAPACITY,
            Producer::claim(Arc::clone(&retired)),
            applied.clone(),
        ));
        let publisher = Self {
            mirror: TopologyMirror::of(&graph.modules, &graph.edges),
            generation: 0,
            block_size: graph.block_size,
            requests: requests.clone(),
            published,
            inputs: Producer::claim(inputs),
            applied,
            closed: false,
        };
        let ends = LinkEnds {
            requests: requests.reserving(EDIT_RESERVE as u64),
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
    /// The caller submits the request before releasing the publisher, so it
    /// is in the queue before any later publication's edit (see
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

    /// Queues a write resolved by [`Self::input_write`] for the next block.
    /// Fails with [`GraphCommandError::QueueFull`] when
    /// the audio thread has not drained earlier writes, and with
    /// [`GraphCommandError::AudioThreadStopped`] when it is gone.
    pub(crate) fn queue_input(&mut self, write: InputWrite) -> Result<(), GraphCommandError> {
        if !self.audio_alive() {
            return Err(GraphCommandError::AudioThreadStopped);
        }
        self.inputs
            .push(write)
            .map_err(|_| GraphCommandError::QueueFull)?;
        Ok(())
    }

    /// Block size publications are compiled for.
    pub(crate) fn block_size(&self) -> usize {
        self.block_size
    }

    /// Whether the audio side of the link still exists.
    pub(crate) fn audio_alive(&self) -> bool {
        !self.closed && Arc::strong_count(&self.applied) > 1
    }

    /// Marks the audio side gone, refusing every later submission. The
    /// request drain calls it under this lock as it is dropped, before its
    /// queue drops what is left: every submission checks
    /// [`Self::audio_alive`] under the same lock, so none can land behind
    /// that last drain and keep the queue alive (a queued edit holds
    /// modules whose controls submit to it).
    pub(crate) fn close(&mut self) {
        self.closed = true;
    }

    /// Publishes a prepared change and returns the mirror it replaced.
    ///
    /// The publication goes to the audio thread as an edit request
    /// ([`Request::edit`]), at once, in the queue control requests share:
    /// requests submitted before it apply to the graph it replaces, those
    /// after it (resolved against the new mirror) to the graph it installs.
    ///
    /// A change prepared before another publication is refused with
    /// [`GraphCommandError::TopologyMoved`]: it was validated against a
    /// topology that no longer exists, so it is never replayed onto the
    /// current one. A full queue refuses it with
    /// [`GraphCommandError::QueueFull`]. Every error returns with nothing
    /// published.
    ///
    /// Nothing the change built is dropped here: a refused change comes back
    /// in [`Refused`], so the caller can drop it (a sink finalizing a file,
    /// say) after releasing the publisher.
    // A refusal is rare and on the control thread; handing the change back
    // whole is the point, so it is not boxed.
    #[allow(clippy::result_large_err)]
    pub(crate) fn publish(&mut self, mut prepared: PreparedChange) -> Result<Published, Refused> {
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
        let Some(mut publication) = prepared.publication.take() else {
            // An empty change publishes nothing.
            return Ok(Published {
                previous: self.mirror.clone(),
                built: prepared.built,
            });
        };
        // Edits install in queue order, so the mirror is the order the
        // audio thread will be running when it installs this one.
        publication.map_survivors(self.mirror.modules.keys().map(String::as_str));
        publication.generation = self.generation + 1;
        // Every request and input write tagged with the current generation
        // was queued under this lock, so before this push, whose release
        // the audio thread's pop acquires.
        let edit = Request::edit(self.generation, Payload::owned(publication));
        if let Err(QueueFull(edit)) = self.requests.submit(edit) {
            let RequestValue::Edit(payload) = edit.value else {
                unreachable!("the queue hands back the request it was given");
            };
            prepared.publication = payload.take_owned().ok();
            return Err(Refused {
                error: GraphCommandError::QueueFull,
                change: prepared,
            });
        }
        self.generation += 1;
        self.published.store(self.generation, Ordering::Relaxed);
        let previous = std::mem::replace(&mut self.mirror, prepared.mirror);
        Ok(Published {
            previous,
            built: prepared.built,
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
    /// [`Publisher::control_target`]), which submits edits to it too.
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
}
