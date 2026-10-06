//! Atomic topology publication: the audio thread's side.
//!
//! The control thread prepares a complete next topology as a [`Publication`]
//! (see `crate::invention::publish`) and puts it in a [`Mailbox`]. At the
//! start of a block the audio thread takes it and installs it in one step:
//! surviving instances move by key into the prepared module map (keeping
//! their phase and state), the derived structures are swapped in, each
//! survivor's feedback carry is copied across (see [`SurvivorRemap`]), and the
//! boxed publication, now holding the old map and old derived structures
//! (including removed and replaced instances), goes back to the control
//! thread on a bounded retire channel to be freed there. Installing never
//! allocates, frees, or locks, and no block ever plays part of a
//! publication.
//!
//! Queued input writes are plain `Copy` records resolved on the control
//! thread to module and port indices (see [`InputWrite`]), so applying one
//! never allocates, frees, or locks either.
//!
//! Both channels stay lock-free on the audio side only while the control
//! side uses `try_send` and `try_recv` exclusively. A control thread parked
//! in a blocking `send` or `recv` makes the audio thread's `try_*` call take
//! the channel's internal waker lock to wake it.

use indexmap::IndexMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use super::compile::CompiledTopology;
use super::mailbox::Mailbox;
use super::{RoutingConnection, SignalGraph};
use crate::invention::runtime::ModuleInstance;
use crate::{GraphModule, Module, MAX_BLOCK};

mod remap;

pub(crate) use remap::SurvivorRemap;

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
    /// Where each running module lands in `modules`, if it survives. Set by
    /// the publisher as it publishes (see [`Self::map_survivors`]).
    pub(crate) remap: SurvivorRemap,
    /// The publisher generation this publication creates. Set by the
    /// publisher as it publishes; a folded publication keeps the newer one.
    pub(crate) generation: u64,
    /// For each generation folded into this one, a remap from that
    /// generation's order into `modules` (see [`Self::absorb`]).
    pub(crate) absorbed: Vec<Absorbed>,
}

/// A generation folded into a newer publication before the audio thread
/// took it, kept so input writes resolved against it still find their
/// instance when the newer one installs. Built and freed on the control
/// thread; the audio thread only reads it.
pub(crate) struct Absorbed {
    pub(crate) generation: u64,
    /// Module ids in that generation's order, for the same defensive id
    /// check survivors get.
    pub(crate) ids: Vec<String>,
    /// From that generation's order to the folding publication's. Unlike
    /// the survivor remap it maps modules a folded publication built too:
    /// those are the instances its writes were resolved against.
    pub(crate) remap: SurvivorRemap,
}

impl Publication {
    /// Number of survivor placeholders the audio thread must fill.
    pub(crate) fn survivor_count(&self) -> usize {
        self.survivor.iter().filter(|&&s| s).count()
    }

    /// Maps the modules of the graph that will be running when this
    /// publication installs (`running`, ids in module order) onto this
    /// publication's survivors. Control thread only: allocates.
    pub(crate) fn map_survivors<'a>(&mut self, running: impl IntoIterator<Item = &'a str>) {
        self.remap = SurvivorRemap::map(running, &self.modules, &self.survivor);
    }

    /// Folds an earlier publication the audio thread never took into this
    /// one. Its prepared instances are the newest version of their modules,
    /// so each fills this publication's survivor placeholder of the same id,
    /// trading places with it. The survivor remaps compose, so this one
    /// maps from the graph still running, which never had the instances the
    /// earlier one built: they start from zero. Returns what is left of the
    /// earlier publication, including prepared instances this one no longer
    /// needs, for the caller to drop once it has released the publisher.
    ///
    /// Before composing, this remap still maps from the earlier
    /// publication's order, so it is kept (with any the earlier one carried,
    /// composed through it) for writes resolved against the earlier
    /// generation. Control thread only: allocates.
    #[must_use = "the superseded publication should be dropped off the publisher lock"]
    pub(crate) fn absorb(&mut self, mut earlier: Box<Publication>) -> Box<Publication> {
        for mut folded in earlier.absorbed.drain(..) {
            folded.remap = folded.remap.then(&self.remap);
            self.absorbed.push(folded);
        }
        self.absorbed.push(Absorbed {
            generation: earlier.generation,
            ids: earlier.modules.keys().cloned().collect(),
            remap: self.remap.clone(),
        });
        let Publication {
            modules, survivor, ..
        } = &mut *earlier;
        for ((id, instance), &was_survivor) in modules.iter_mut().zip(survivor.iter()) {
            if was_survivor {
                continue;
            }
            if let Some((idx, _, slot)) = self.modules.get_full_mut(id.as_str()) {
                if self.survivor[idx] {
                    std::mem::swap(slot, instance);
                    self.survivor[idx] = false;
                }
            }
        }
        self.remap.compose_after(&earlier.remap, &self.survivor);
        earlier
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

/// Longest input port name a queued write can target. Applying a write
/// copies the port's name into a stack buffer of this size (see
/// [`SignalGraph::apply_input`]); the control side refuses longer names.
pub(crate) const MAX_INPUT_PORT_NAME: usize = 128;

/// A direct write to a module's input port, delivered at the next block.
///
/// Resolved on the control thread, under the publisher lock, against the
/// publisher's mirror as of `generation`: `module_idx` is the module's
/// position in that mirror, which is the audio graph's module order once
/// that generation is installed, and `port_idx` its position in the
/// module's inputs. Plain data, so receiving and applying one never
/// allocates or frees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InputWrite {
    pub(crate) generation: u64,
    pub(crate) module_idx: usize,
    pub(crate) port_idx: usize,
    pub(crate) value: f32,
}

/// What the audio thread does with a queued write this block.
enum Disposition {
    /// Apply it at this module index of the running graph.
    Apply(usize),
    /// Keep it: it targets a publication not yet installed.
    Hold,
    /// Drop it: its target went away, or its order can no longer be mapped.
    Drop,
}

/// The audio thread's end of a live graph's link to the control thread.
///
/// It tracks the generation of the installed publication, so each queued
/// input write (tagged with the generation it was resolved against) is
/// applied to the right instance:
///
/// - tagged with the installed generation: applied at its index;
/// - tagged with the generation installed just before this block's install,
///   or one folded into the publication it installs: mapped into the new
///   order (through the survivor remap or [`Absorbed`]), and dropped when
///   its module was removed or rebuilt (its target went away);
/// - tagged with a newer generation: its publication is published but not
///   yet installed (a retirement is held, say), so it waits in a fixed,
///   preallocated ring and is applied right after that install. When the
///   ring is full the channel is left undrained, so the control side sees
///   [`crate::invention::runtime::GraphCommandError::QueueFull`] rather than
///   a write being lost;
/// - anything else: dropped. The publisher's generations are consecutive
///   and every one is installed or folded, so this is only a defensive
///   fallback.
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
    /// The generation of the installed publication; the publisher starts
    /// at 0 with the graph it linked.
    installed: u64,
    /// Writes for a publication not yet installed, oldest first. Allocated
    /// once at link time and never pushed past its capacity.
    pending: Vec<InputWrite>,
}

impl AudioLink {
    /// Links a graph to its publisher. `inputs` must be a bounded
    /// (`sync_channel`) receiver of `input_capacity`: receiving from it
    /// never frees, and the ring for writes awaiting a publication is
    /// allocated here at the same size. The control side must only
    /// `try_send` on `inputs` and `try_recv` on `retire`'s receiver (see the
    /// module docs).
    pub(crate) fn new(
        publications: Arc<Mailbox<Publication>>,
        inputs: Receiver<InputWrite>,
        input_capacity: usize,
        retire: SyncSender<Box<Publication>>,
        applied: Arc<AtomicU64>,
    ) -> Self {
        Self {
            publications,
            inputs,
            retire,
            held: None,
            applied,
            installed: 0,
            pending: Vec::with_capacity(input_capacity.max(1)),
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
    /// Installs a pending publication and applies queued input writes (see
    /// [`AudioLink`] for which instance each write reaches). Writes are
    /// applied after the install, so they survive its input reset (though
    /// not the full reset when an install falls back to recompiling, which
    /// `ensure_process_order` runs afterwards), and before the retired
    /// publication goes back, so its remaps are in hand. Takes at most the
    /// ring's capacity from the channel per block, so a sender keeping pace
    /// cannot hold the block here.
    /// Allocation-, free-, and lock-free; runs at the start of a block.
    pub(super) fn drain_link(&mut self) {
        let Some(mut link) = self.link.take() else {
            return;
        };
        let previous = link.installed;
        let mut taken: Option<Box<Publication>> = None;
        if link.flush_held() {
            if let Some(mut publication) = link.publications.take() {
                self.install(&mut publication);
                link.installed = publication.generation;
                link.applied.fetch_add(1, Ordering::Relaxed);
                taken = Some(publication);
            }
        }
        let retired = taken.as_deref().map(|retired| (previous, retired));
        let installed = link.installed;
        // Held writes came off the channel first, so they go first.
        link.pending
            .retain(|write| match self.dispose(write, installed, retired) {
                Disposition::Apply(module_idx) => {
                    self.apply_input(module_idx, write.port_idx, write.value);
                    false
                }
                Disposition::Hold => true,
                Disposition::Drop => false,
            });
        // A full ring leaves the rest in the channel: every write behind a
        // held one is at least as new, so it would be held too.
        let limit = link.pending.capacity();
        for _ in 0..limit {
            if link.pending.len() == limit {
                break;
            }
            let Ok(write) = link.inputs.try_recv() else {
                break;
            };
            match self.dispose(&write, installed, retired) {
                Disposition::Apply(module_idx) => {
                    self.apply_input(module_idx, write.port_idx, write.value)
                }
                Disposition::Hold => link.pending.push(write),
                Disposition::Drop => {}
            }
        }
        if let Some(publication) = taken {
            link.retire(publication);
        }
        self.link = Some(link);
    }

    /// Decides where `write` goes, given the installed generation and, in a
    /// block that installed, the generation installed before it with the
    /// retired publication (whose remap maps from that generation's order).
    fn dispose(
        &self,
        write: &InputWrite,
        installed: u64,
        retired: Option<(u64, &Publication)>,
    ) -> Disposition {
        if write.generation == installed {
            return Disposition::Apply(write.module_idx);
        }
        if write.generation > installed {
            return Disposition::Hold;
        }
        let Some((previous, retired)) = retired else {
            return Disposition::Drop;
        };
        let old = write.module_idx;
        let target = if write.generation == previous {
            // The retired map is the previous generation's order.
            let old_id = retired.modules.get_index(old).map(|(id, _)| id);
            retired.remap.get(old).map(|new| (old_id, new))
        } else {
            retired
                .absorbed
                .iter()
                .find(|folded| folded.generation == write.generation)
                .and_then(|folded| folded.remap.get(old).map(|new| (folded.ids.get(old), new)))
        };
        // The same defensive id check as `carry_survivors`.
        match target {
            Some((Some(old_id), new))
                if self
                    .modules
                    .get_index(new)
                    .is_some_and(|(id, _)| id == old_id) =>
            {
                Disposition::Apply(new)
            }
            _ => Disposition::Drop,
        }
    }

    /// Sets input `port_idx` of the module at `module_idx` to `value`,
    /// skipping indices out of range. The port's name is copied into a stack
    /// buffer, since `Module::set_input` takes a name and the module cannot
    /// stay borrowed for it. Allocation-free as long as the module's own
    /// `set_input` is.
    fn apply_input(&mut self, module_idx: usize, port_idx: usize, value: f32) {
        let Some((_, instance)) = self.modules.get_index_mut(module_idx) else {
            return;
        };
        let module = instance.module_mut();
        let mut name = [0u8; MAX_INPUT_PORT_NAME];
        let len = match module.inputs().get(port_idx) {
            Some(port) if port.len() <= MAX_INPUT_PORT_NAME => {
                name[..port.len()].copy_from_slice(port.as_bytes());
                port.len()
            }
            _ => return,
        };
        if let Ok(port) = std::str::from_utf8(&name[..len]) {
            let _ = module.set_input(port, value);
        }
    }

    /// Swaps a prepared publication in. Afterwards `publication` holds the
    /// previous module map (with removed and replaced instances, and vacant
    /// placeholders where survivors were) and the previous derived state.
    /// Survivors keep their feedback carry; added and rebuilt modules start
    /// from zero. A loop is sample-identical across the install as long as
    /// the edit leaves its per-sample order (and so its delayed edge) as it
    /// was.
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
        // indexes a placeholder's missing ports. Recompiling resets every
        // carry, so the fallback copies none.
        if moved != expected || self.block_capacity != self.block_size.clamp(1, MAX_BLOCK) {
            self.topo_dirty = true;
        } else {
            self.carry_survivors(publication);
        }
        self.reset_inputs();
    }

    /// Copies each survivor's previous-block outputs from the retired
    /// topology into the installed one, by the publication's survivor remap.
    /// Copies f32s between buffers both sides already own, so it never
    /// allocates or frees. Defensive: a remap that does not cover the
    /// retired map carries nothing, and an entry whose ids or output port
    /// counts disagree is skipped (left at zero) rather than trusted.
    fn carry_survivors(&mut self, retired: &Publication) {
        if retired.remap.len() != retired.modules.len() {
            return;
        }
        for (old, new) in retired.remap.survivors() {
            let same_module = match (retired.modules.get_index(old), self.modules.get_index(new)) {
                (Some((old_id, _)), Some((new_id, _))) => old_id == new_id,
                _ => false,
            };
            if !same_module {
                continue;
            }
            let (Some(src), Some(dst)) = (
                retired.topology.out_prev.get(old),
                self.out_prev.get_mut(new),
            ) else {
                continue;
            };
            // Each carry holds one value per output port.
            if src.len() == dst.len() {
                dst.copy_from_slice(src);
            }
        }
    }
}

#[cfg(test)]
mod tests;
