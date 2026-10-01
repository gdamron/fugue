//! Atomic topology publication: the audio thread's side.
//!
//! The control thread prepares a complete next topology as a [`Publication`]
//! (see `crate::invention::publish`) and puts it in a [`Mailbox`]. At the
//! start of a block the audio thread takes it and installs it in one step:
//! surviving instances move by key into the prepared module map (keeping
//! their phase and state), the derived structures are swapped in, and the
//! boxed publication, now holding the old map and old derived structures
//! (including removed and replaced instances), goes back to the control
//! thread on a bounded retire channel to be freed there. Nothing on this
//! path allocates, frees, or locks, and no block ever plays part of a
//! publication.

use indexmap::IndexMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use super::compile::CompiledTopology;
use super::mailbox::Mailbox;
use super::{RoutingConnection, SignalGraph};
use crate::invention::runtime::ModuleInstance;
use crate::{GraphModule, Module, MAX_BLOCK};

/// A complete next topology, prepared off the audio thread.
pub(crate) struct Publication {
    /// The next module map in final order. Prepared instances (new or
    /// replacement) are in place; each surviving module holds a vacant
    /// placeholder until the audio thread moves the running instance in.
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Whether each entry of `modules` (by index) is a survivor placeholder.
    pub(crate) survivor: Vec<bool>,
    /// Sink module ids, in module order.
    pub(crate) sinks: Vec<String>,
    /// The string-keyed edge list the topology was compiled from.
    pub(crate) edges: Vec<RoutingConnection>,
    /// Derived structures, sized for the graph's block size.
    pub(crate) topology: CompiledTopology,
}

impl Publication {
    /// Number of survivor placeholders the audio thread must fill.
    pub(crate) fn survivor_count(&self) -> usize {
        self.survivor.iter().filter(|&&s| s).count()
    }

    /// Folds an earlier publication the audio thread never took into this
    /// one. Its prepared instances are the newest version of their modules,
    /// so each fills this publication's survivor placeholder of the same id;
    /// prepared instances this publication no longer needs are dropped here,
    /// on the control thread.
    pub(crate) fn absorb(&mut self, earlier: Box<Publication>) {
        let Publication {
            modules, survivor, ..
        } = *earlier;
        for ((id, instance), was_survivor) in modules.into_iter().zip(survivor) {
            if was_survivor {
                continue;
            }
            if let Some((idx, _, slot)) = self.modules.get_full_mut(&id) {
                if self.survivor[idx] {
                    *slot = instance;
                    self.survivor[idx] = false;
                }
            }
        }
    }
}

/// A port-less, state-less module holding a survivor's slot in a prepared
/// map. Zero-sized, so boxing and dropping it never touch the allocator.
struct Vacant;

impl Module for Vacant {
    fn name(&self) -> &str {
        "vacant"
    }

    fn process(&mut self, _frames: usize) -> bool {
        false
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &[]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &[]
    }

    fn set_input(&mut self, _port: &str, _value: f32) -> Result<(), String> {
        Err(String::new())
    }

    fn get_output(&self, _port: &str) -> Result<f32, String> {
        Err(String::new())
    }
}

/// A placeholder for a surviving module in a prepared map.
pub(crate) fn vacant() -> ModuleInstance {
    GraphModule::Module(Box::new(Vacant))
}

/// A direct write to a module's input port, delivered at the next block.
pub(crate) struct InputWrite {
    pub(crate) module_id: String,
    pub(crate) port: String,
    pub(crate) value: f32,
}

/// The audio thread's end of a live graph's link to the control thread.
pub(crate) struct AudioLink {
    publications: Arc<Mailbox<Publication>>,
    inputs: Receiver<InputWrite>,
    retire: SyncSender<Box<Publication>>,
    /// A retirement the retire channel had no room for. While it is held no
    /// further publication is taken, so the audio thread never holds more
    /// than this one; it is sent as soon as the channel has room.
    held: Option<Box<Publication>>,
    /// Publications installed so far, for observation off the audio thread.
    applied: Arc<AtomicU64>,
}

impl AudioLink {
    /// Links a graph to its publisher. `inputs` must be a bounded
    /// (`sync_channel`) receiver: receiving from it never frees.
    pub(crate) fn new(
        publications: Arc<Mailbox<Publication>>,
        inputs: Receiver<InputWrite>,
        retire: SyncSender<Box<Publication>>,
        applied: Arc<AtomicU64>,
    ) -> Self {
        Self {
            publications,
            inputs,
            retire,
            held: None,
            applied,
        }
    }

    /// Sends a held retirement if there is room; true when none is held.
    fn flush_held(&mut self) -> bool {
        match self.held.take() {
            None => true,
            Some(retired) => {
                self.retire(retired);
                self.held.is_none()
            }
        }
    }

    /// Hands a publication back to the control thread, holding it when the
    /// channel is full (or the control side is gone) rather than freeing it.
    fn retire(&mut self, retired: Box<Publication>) {
        match self.retire.try_send(retired) {
            Ok(()) => {}
            Err(TrySendError::Full(retired)) | Err(TrySendError::Disconnected(retired)) => {
                self.held = Some(retired);
            }
        }
    }
}

impl SignalGraph {
    /// Installs a pending publication and applies queued input writes.
    /// Allocation-, free-, and lock-free; runs at the start of a block.
    pub(super) fn drain_link(&mut self) {
        let Some(mut link) = self.link.take() else {
            return;
        };
        if link.flush_held() {
            if let Some(mut publication) = link.publications.take() {
                self.install(&mut publication);
                link.applied.fetch_add(1, Ordering::Relaxed);
                link.retire(publication);
            }
        }
        while let Ok(write) = link.inputs.try_recv() {
            if let Some(module) = self.modules.get_mut(write.module_id.as_str()) {
                let _ = module.module_mut().set_input(&write.port, write.value);
            }
            // The write's strings are freed here: a remaining audio-thread
            // free, bounded by the input channel's capacity per block.
        }
        self.link = Some(link);
    }

    /// Swaps a prepared publication in. Afterwards `publication` holds the
    /// previous module map (with removed and replaced instances, and vacant
    /// placeholders where survivors were) and the previous derived state.
    fn install(&mut self, publication: &mut Publication) {
        let mut moved = 0;
        for (id, instance) in self.modules.iter_mut() {
            if let Some((idx, _, slot)) = publication.modules.get_full_mut(id.as_str()) {
                if publication.survivor[idx] {
                    std::mem::swap(slot, instance);
                    moved += 1;
                }
            }
        }
        let expected = publication.survivor_count();

        std::mem::swap(&mut self.modules, &mut publication.modules);
        std::mem::swap(&mut self.sinks, &mut publication.sinks);
        std::mem::swap(&mut self.edges, &mut publication.edges);
        publication.topology.swap_with(self);

        // A survivor missing from the running map (which the publisher's
        // mirror rules out) or buffers sized for another block size fall back
        // to recompiling from the instances themselves, so the hot path never
        // indexes a placeholder's missing ports.
        if moved != expected || self.block_capacity != self.block_size.clamp(1, MAX_BLOCK) {
            self.topo_dirty = true;
        }
        self.reset_inputs();
    }
}
