//! The single control-thread publisher of a live graph's topology.

use indexmap::IndexMap;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;

use super::change::{BuiltModule, PreparedChange, TopologyMirror};
use crate::invention::graph::{AudioLink, InputWrite, Mailbox, Publication, SignalGraph};
use crate::invention::runtime::GraphCommandError;

/// Input writes that may wait for the audio thread before a write is
/// refused with [`GraphCommandError::QueueFull`].
pub(crate) const INPUT_QUEUE_CAPACITY: usize = 256;

/// Retired publications the audio thread may hand back before the control
/// thread frees them. The audio thread takes at most one publication per
/// block and the reclaimer drains the channel on a short cadence and before
/// every change, so this is rarely more than one deep; beyond it the audio
/// thread holds one more retirement and stops taking publications until
/// there is room.
pub(crate) const RETIRE_CAPACITY: usize = 8;

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
    publications: Arc<Mailbox<Publication>>,
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
        let (inputs, input_rx) = mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
        let (retire_tx, retired) = mpsc::sync_channel(RETIRE_CAPACITY);
        let applied = Arc::new(AtomicU64::new(0));
        graph.link = Some(AudioLink::new(
            publications.clone(),
            input_rx,
            retire_tx,
            applied.clone(),
        ));
        let publisher = Self {
            mirror: TopologyMirror::of(&graph.modules, &graph.edges),
            generation: 0,
            block_size: graph.block_size,
            publications,
            applied,
        };
        (publisher, LinkEnds { inputs, retired })
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

    /// Block size publications are compiled for.
    pub(crate) fn block_size(&self) -> usize {
        self.block_size
    }

    /// Whether the audio side of the link still exists.
    fn audio_alive(&self) -> bool {
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
        let superseded = self
            .publications
            .take()
            .map(|pending| publication.absorb(pending));
        // Only this publisher puts, under its lock, so the slot is empty.
        drop(self.publications.put(publication));
        self.generation += 1;
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

/// The control side's ends of a link's channels. Use only `try_send` on
/// `inputs` and `try_recv` on `retired`: a control thread blocked on either
/// would make the audio thread's end take the channel's waker lock.
pub(crate) struct LinkEnds {
    /// Queues direct input writes for the next block.
    pub(crate) inputs: SyncSender<InputWrite>,
    /// Retired publications, to free off the audio thread.
    pub(crate) retired: Receiver<Box<Publication>>,
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
