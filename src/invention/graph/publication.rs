//! Atomic topology publication: the audio thread's side.
//!
//! The control thread prepares a complete next topology as a [`Publication`]
//! (see `crate::invention::publish`) and submits it as an edit request, in
//! the queue control requests share. When the audio thread's request drain
//! reaches it, at the start of a block, it installs it in one step:
//! surviving instances move by key into the prepared module map (keeping
//! their phase and state), the derived structures are swapped in, each
//! survivor's feedback carry is copied across and only its inputs whose
//! connectivity changed are reset (see [`SurvivorRemap`]), and the boxed
//! publication, now holding the old map and old derived structures
//! (including removed and replaced instances), goes back to the control
//! thread on a bounded retire ring to be freed there. Installing never
//! allocates, frees, or locks, and no block ever plays part of a
//! publication.
//!
//! Queued input writes are plain `Copy` records resolved on the control
//! thread to module and port indices (see [`InputWrite`]), so applying one
//! never allocates, frees, or locks either.
//!
//! Writes and retirements travel on [`crate::spsc`] rings, whose
//! audio-side ends (a claimed [`Consumer`] for writes, the [`Producer`] for
//! retirements) allocate, lock and wait for nothing, even while the
//! control-side peer is preempted mid-operation. (std's bounded channel can
//! spin or yield then.)

use indexmap::IndexMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::compile::CompiledTopology;
use super::{RoutingConnection, SignalGraph};
use crate::invention::runtime::ModuleInstance;
use crate::spsc::{Consumer, Producer};
use crate::{GraphModule, Module, MAX_BLOCK};

mod handover;
mod landing;
mod remap;

pub(crate) use handover::Handover;
pub(crate) use landing::FateGuard;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use landing::{Fate, TIMED_EDIT_MODULE};
pub(crate) use remap::SurvivorRemap;

/// A complete next topology, prepared off the audio thread.
pub(crate) struct Publication {
    /// The next module map in final order. Prepared instances (new or
    /// replacement) are in place; each surviving module holds a vacant
    /// placeholder until the audio thread moves the running instance in.
    pub(crate) modules: IndexMap<String, ModuleInstance>,
    /// Whether each entry of `modules` (by index) is a survivor placeholder.
    pub(crate) survivor: Vec<bool>,
    /// Whether each entry of `modules` (by index) took over from the running
    /// instance it replaces (see [`Module::inherit`]). All false until the
    /// audio thread installs the publication; prepared at full length, so
    /// marking an entry never allocates.
    pub(crate) inherited: Vec<bool>,
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
    /// publisher as it publishes.
    pub(crate) generation: u64,
    /// Payloads for surviving modules, applied as it installs (see
    /// [`Handover`]). Set by the commit.
    pub(crate) handovers: Vec<Handover>,
    /// A timed edit's fate, which landing it decides (see `landing`); none
    /// for an immediate edit.
    pub(crate) fate: Option<FateGuard>,
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

/// What the audio thread does with a queued write or request this block.
pub(super) enum Disposition {
    /// Apply it at this module index of the running graph.
    Apply(usize),
    /// Keep it: it targets a publication not yet installed.
    Hold,
    /// Drop it: its target went away, or its order can no longer be mapped.
    Drop,
}

/// Edits the audio thread installs in one block at most. Edits queue one
/// per change and the reclaimer frees retirements on a short cadence, so a
/// block rarely meets more than one; this only bounds the block's work.
pub(crate) const MAX_INSTALLS_PER_BLOCK: usize = 8;

/// The audio thread's end of a live graph's link to the control thread.
///
/// It tracks the generation of the installed publication, so each queued
/// input write (tagged with the generation it was resolved against) is
/// applied to the right instance:
///
/// - tagged with the installed generation: applied at its index;
/// - tagged with the generation an install just replaced: mapped into the
///   new order through its survivor remap, and dropped when its module was
///   removed or rebuilt (its target went away);
/// - tagged with a newer generation: its publication is published but not
///   yet installed (a retirement is held, say), so it waits in a fixed,
///   preallocated ring and is applied right after that install. When the
///   ring is full the queue is left undrained, so the control side sees
///   [`crate::invention::runtime::GraphCommandError::QueueFull`] rather than
///   a write being lost;
/// - anything else: dropped. The publisher's generations are consecutive
///   and the writes are mapped through every install, so this is only a
///   defensive fallback.
pub(crate) struct AudioLink {
    inputs: Consumer<InputWrite>,
    retire: Producer<Box<Publication>>,
    /// A retirement the retire ring had no room for. While it is held no
    /// further edit is installed, so the audio thread never holds more
    /// than this one; it is sent as soon as the ring has room.
    held: Option<Box<Publication>>,
    /// Publications installed so far, for observation off the audio thread.
    /// The publisher holds the other reference, and reads its count to tell
    /// whether this side still exists.
    applied: Arc<AtomicU64>,
    /// The generation of the installed publication; the publisher starts
    /// at 0 with the graph it linked.
    installed: u64,
    /// Writes for a publication not yet installed, oldest first. Allocated
    /// once at link time and never pushed past its capacity.
    pending: Vec<InputWrite>,
    /// A timed edit taken at this segment start, to install there.
    landed: Option<Box<Publication>>,
    /// The publication a timed edit's landing retired, kept to map what
    /// was submitted against the generation it replaced (see `landing`).
    bridge: Option<Box<Publication>>,
}

impl AudioLink {
    /// Links a graph to its publisher. `inputs` is the claimed consumer of
    /// a ring of `input_capacity` writes; the ring for writes awaiting a
    /// publication is allocated here at the same size. `retire` hands
    /// retired publications to the control thread. Publications themselves
    /// arrive as edits in the graph's request queue.
    pub(crate) fn new(
        inputs: Consumer<InputWrite>,
        input_capacity: usize,
        retire: Producer<Box<Publication>>,
        applied: Arc<AtomicU64>,
    ) -> Self {
        Self {
            inputs,
            retire,
            held: None,
            applied,
            installed: 0,
            pending: Vec::with_capacity(input_capacity.max(1)),
            landed: None,
            bridge: None,
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
    /// ring is full rather than freeing it.
    fn retire(&mut self, retired: Box<Publication>) {
        if let Err(retired) = self.retire.push(retired) {
            self.held = Some(retired);
        }
    }
}

impl SignalGraph {
    /// Drains the request queue, which installs every edit it reaches in
    /// queue order, and applies queued input writes (see [`AudioLink`] for
    /// which instance each write reaches): through each install, with its
    /// retired publication in hand, then once more for writes to the graph
    /// as installed. Writes are applied after an install, so its input
    /// reset never clears one, and before the retired publication goes
    /// back, so its remaps are in hand. A written port that stays
    /// unconnected keeps its value through later installs too, including
    /// one that falls back to recompiling (which `ensure_process_order`
    /// runs afterwards; see [`SignalGraph::recompile`]).
    ///
    /// A timed edit's bridge (see `landing`) goes back first, once the
    /// publisher has adopted its edit and nothing is left to map through
    /// it; until then no further edit installs, nor while it waits for
    /// room to go back.
    ///
    /// An edit installs only while no retirement is held, so the audio
    /// thread never holds more than one, and at most
    /// [`MAX_INSTALLS_PER_BLOCK`] install per block, however fast the
    /// reclaimer makes room; the queue behind them waits.
    /// Allocation-, free-, and lock-free; runs at the start of a block.
    pub(super) fn drain_link(&mut self) {
        self.with_link(Self::drain_link_with);
    }

    /// [`Self::drain_link`] with the link taken out of the graph, and each
    /// edit's publication held `in_hand` from its pop until it retires (or
    /// the bridge, while writes map through it).
    fn drain_link_with(&mut self, link: &mut AudioLink, in_hand: &mut Option<Box<Publication>>) {
        let mut budget = self.requests.as_ref().map_or(0, |drain| drain.pop_limit);
        let mut installs = 0;
        // Read before anything is drained (see `landing`).
        let adopted = link.bridge.is_some()
            && self
                .requests
                .as_ref()
                .is_some_and(|drain| drain.adopted(link.installed));
        loop {
            // A bridge goes before the next edit installs, so the link
            // never holds two publications back.
            let can_install =
                installs < MAX_INSTALLS_PER_BLOCK && link.bridge.is_none() && link.flush_held();
            let drained =
                self.drain_requests(link.installed, can_install, &mut budget, link.bridge());
            let Some(publication) = drained else {
                if self.release_bridge(link, in_hand, adopted) {
                    continue;
                }
                break;
            };
            let publication = in_hand.insert(publication);
            let previous = link.installed;
            self.install(publication);
            link.installed = publication.generation;
            link.applied.fetch_add(1, Ordering::Relaxed);
            let retired = Some((previous, &**publication));
            self.remap_requests(link.installed, retired);
            self.drain_inputs(link, retired);
            if let Some(publication) = in_hand.take() {
                link.retire(publication);
            }
            installs += 1;
        }
        // In hand, as the edits are, while the writes apply.
        *in_hand = link.bridge.take();
        let retired = in_hand
            .as_deref()
            .map(|bridge| (bridge.generation - 1, bridge));
        self.drain_inputs(link, retired);
        link.bridge = in_hand.take();
    }

    /// Applies queued input writes for the installed generation, maps those
    /// for the one `retired` replaced, and holds those for a newer one.
    /// Takes at most the ring's capacity from the queue per call, so a
    /// sender keeping pace cannot hold the block here. Returns whether
    /// nothing for the installed generation or an older one is left in the
    /// queue: it emptied, or holds a write for a newer one, so everything
    /// behind it is newer too.
    fn drain_inputs(&mut self, link: &mut AudioLink, retired: Option<(u64, &Publication)>) -> bool {
        let installed = link.installed;
        // Held writes came off the queue first, so they go first.
        link.pending.retain(|write| {
            match self.dispose(write.generation, write.module_idx, installed, retired) {
                Disposition::Apply(module_idx) => {
                    self.apply_input(module_idx, write.port_idx, write.value);
                    false
                }
                Disposition::Hold => true,
                Disposition::Drop => false,
            }
        });
        // A full ring leaves the rest in the queue: every write behind a
        // held one is at least as new, so it would be held too.
        let limit = link.pending.capacity();
        for _ in 0..limit {
            if link.pending.len() == limit {
                return true;
            }
            let Some(write) = link.inputs.pop() else {
                return true;
            };
            match self.dispose(write.generation, write.module_idx, installed, retired) {
                Disposition::Apply(module_idx) => {
                    self.apply_input(module_idx, write.port_idx, write.value)
                }
                Disposition::Hold => link.pending.push(write),
                Disposition::Drop => {}
            }
        }
        !link.pending.is_empty()
    }

    /// Decides where a write or request resolved against module `old` of
    /// `generation` goes, given the installed generation and, in a block
    /// that installed, the generation installed before it with the retired
    /// publication (whose remap maps from that generation's order).
    pub(super) fn dispose(
        &self,
        generation: u64,
        old: usize,
        installed: u64,
        retired: Option<(u64, &Publication)>,
    ) -> Disposition {
        if generation == installed {
            return Disposition::Apply(old);
        }
        if generation > installed {
            return Disposition::Hold;
        }
        let Some((previous, retired)) = retired else {
            return Disposition::Drop;
        };
        if generation != previous {
            return Disposition::Drop;
        }
        // The retired map is the previous generation's order.
        let old_id = retired.modules.get_index(old).map(|(id, _)| id);
        let target = retired.remap.get(old).map(|new| (old_id, new));
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
    pub(super) fn apply_input(&mut self, module_idx: usize, port_idx: usize, value: f32) {
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

    /// Installs `publication` in an offline render's graph in place, as the
    /// audio thread installs a live one (see [`Self::install`]), and counts
    /// the edit, so a payload resolved before it is refused as stale (see
    /// `declared::Checked`). Every pending request, watch, ramp and
    /// condition follows its module to its new index, or is refused
    /// ([`Refusal::TargetGone`](crate::control_request::Refusal)) when the
    /// edit removed or rebuilt it, as through a live install. First, what
    /// is due at this sample plays on the graph it replaces, as before a
    /// live install of an immediate edit, or of a timed one as it lands
    /// (`landing`). Returns `publication` holding what the install
    /// displaced, to drop off the graph's lock. Allocates: offline.
    pub(crate) fn install_offline(
        &mut self,
        publication: Box<Publication>,
        landing: bool,
    ) -> Box<Publication> {
        let installed = self.requests.as_ref().map_or(0, |drain| drain.installed);
        let mut publication = self.play_before_offline_install(publication, landing);
        self.install(&mut publication);
        // The render's own generations: what was resolved against the
        // order just retired maps through its remap.
        self.remap_requests(installed + 1, Some((installed, &*publication)));
        // No publisher keeps an ordering here: a recompile in place (an
        // offline retarget, say) reads the modules' targets afresh.
        self.published = false;
        self.offline_edits += 1;
        if let Some(watch) = &mut self.end_watch {
            watch.refresh(&self.modules);
        }
        publication
    }

    /// Swaps a prepared publication in. Afterwards `publication` holds the
    /// previous module map (with removed and replaced instances, and vacant
    /// placeholders where survivors were) and the previous derived state.
    /// Survivors keep their feedback carry and every input that stays
    /// unconnected, with any value written to it; added and rebuilt modules
    /// start from zero, though a rebuilt one may first take over what the
    /// instance it replaces was playing (see [`Module::inherit`]), and then
    /// keeps that instance's feedback carry as a survivor does. A loop is
    /// sample-identical across the install as long as the edit leaves its
    /// per-sample order (and so its delayed edge) as it was. Last, each
    /// survivor takes the payloads the publication carries for it (see
    /// [`Handover`]).
    fn install(&mut self, publication: &mut Publication) {
        let mut moved = 0;
        for (id, instance) in self.modules.iter_mut() {
            if let Some((idx, _, slot)) = publication.modules.get_full_mut(id.as_str()) {
                if publication.survivor[idx] {
                    std::mem::swap(slot, instance);
                    moved += 1;
                } else if slot.module_mut().inherit(instance.module()) {
                    // Rebuilt in place, carrying on from what the running
                    // instance was playing: its feedback carry follows too.
                    publication.inherited[idx] = true;
                }
            }
        }
        let expected = publication.survivor_count();

        std::mem::swap(&mut self.modules, &mut publication.modules);
        std::mem::swap(&mut self.sinks, &mut publication.sinks);
        std::mem::swap(&mut self.edges, &mut publication.edges);
        publication.topology.swap_with(self);
        self.published = true;

        // A survivor missing from the running map (which the publisher's
        // mirror rules out) or buffers sized for another block size fall back
        // to recompiling from the instances themselves, so the hot path never
        // indexes a placeholder's missing ports. Recompiling resets every
        // carry, so the fallback copies none. It keeps the inputs, though:
        // the module order is already the installed one, so it resets only
        // ports whose connectivity it changes. Recompiling allocates on the
        // audio thread, so each fallback is counted in the backend's
        // `AudioDiagnostics` for status readers.
        if moved != expected || self.block_capacity != self.block_size.clamp(1, MAX_BLOCK) {
            self.topo_dirty = true;
            if let Some(diagnostics) = &self.diagnostics {
                diagnostics.record_graph_rebuild();
            }
        } else {
            self.carry_survivors(publication);
            self.carry_inherited(publication);
        }
        self.reset_installed_inputs(publication, moved);
        self.hand_over(publication);
    }

    /// Whether the running module at `old` in the retired map and the
    /// installed module at `new` have the same id: the defensive check every
    /// use of a remap makes before trusting an entry.
    fn same_module(
        &self,
        retired: &IndexMap<String, ModuleInstance>,
        old: usize,
        new: usize,
    ) -> bool {
        match (retired.get_index(old), self.modules.get_index(new)) {
            (Some((old_id, _)), Some((new_id, _))) => old_id == new_id,
            _ => false,
        }
    }

    /// Brings every installed module's inputs in line with the installed
    /// connectivity (see [`Self::reset_module_inputs`]). Each survivor is
    /// compared with its connectivity in the retired topology, by the
    /// survivor remap, so its inputs that stay unconnected keep their
    /// values; every other module starts fresh. Defensive: unless the remap
    /// covers the retired map and vouches, id for id, for every one of the
    /// `moved` survivors, every module starts fresh. Allocation-free.
    fn reset_installed_inputs(&mut self, retired: &Publication, moved: usize) {
        let remap = &retired.remap;
        let vouched = remap.len() == retired.modules.len()
            && remap.survivors().count() == moved
            && remap
                .survivors()
                .all(|(old, new)| self.same_module(&retired.modules, old, new));
        for mi in 0..self.modules.len() {
            let survivor = retired.survivor.get(mi).copied().unwrap_or(false);
            if !(vouched && survivor) {
                self.reset_module_inputs(mi, None);
            }
        }
        if vouched {
            for (old, new) in remap.survivors() {
                let was = retired.topology.connected_in_ports.get(old);
                self.reset_module_inputs(new, was.map(Vec::as_slice));
            }
        }
    }

    /// Copies the previous-block outputs of each instance a rebuilt module
    /// took over from (see [`Module::inherit`]) into the rebuilt module's
    /// carry, so a delayed edge out of it reads what was last played rather
    /// than silence. Pairs them by id; allocation- and free-free. An entry
    /// whose output port counts disagree is skipped (left at zero).
    fn carry_inherited(&mut self, retired: &Publication) {
        for (new, _) in retired.inherited.iter().enumerate().filter(|(_, &i)| i) {
            let old = self
                .modules
                .get_index(new)
                .and_then(|(id, _)| retired.modules.get_index_of(id.as_str()));
            let (Some(src), Some(dst)) = (
                old.and_then(|old| retired.topology.out_prev.get(old)),
                self.out_prev.get_mut(new),
            ) else {
                continue;
            };
            if src.len() == dst.len() {
                dst.copy_from_slice(src);
            }
        }
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
            if !self.same_module(&retired.modules, old, new) {
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
