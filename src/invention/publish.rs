//! Atomic, allocation-free structural changes to a live graph.
//!
//! Every structural change to a running invention (reload, a script's or
//! agent's edit, a single add/remove/swap/connect/disconnect) goes through
//! one [`Publisher`], in three steps:
//!
//! 1. **Prepare** (control thread, all fallible work): a [`GraphChange`]
//!    builds instances, validates edits, attaches schedulers, and compiles
//!    the complete next topology. Nothing visible changes.
//! 2. **Publish**: the prepared topology goes to the audio thread as one
//!    publication, installed at the start of one block without allocating,
//!    freeing, or locking (see `graph::publication`). The old structures come
//!    back to the control thread to be freed.
//! 3. **Commit** (control thread): the runtime's mirrors (modules,
//!    connections, ports, control surfaces, document) update together; this
//!    cannot fail. Control writes made alongside a change (a reload's
//!    config-as-control updates) are validated in step 1, but a write can
//!    still fail when made if only making it reveals the problem (a sample
//!    that does not load). The caller reports such a write; the published
//!    topology stands.
//!
//! Offline render owns its graph outright and keeps applying edits directly.

use indexmap::IndexMap;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use std::sync::mpsc::{SyncSender, TrySendError};

use super::graph::{InputWrite, RoutingConnection, SignalGraph};
use super::orchestration::ModulePorts;
use super::runtime::{ControlSurfaceInstance, GraphCommandError};
use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState};
use crate::control_request::RequestSender;
use crate::ModuleRegistry;

mod change;
mod publisher;
mod reclaim;
mod registry;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use change::{BuiltModule, GraphChange, PreparedChange};
pub(crate) use publisher::{Publisher, Refused};
pub(crate) use reclaim::Reclaimer;
use registry::LiveRegistry;

/// How many times an edit that builds a module ([`LiveGraph::add_module`],
/// [`LiveGraph::swap_module`]) builds it when commits keep adopting new
/// registries underneath it.
const BUILD_ATTEMPTS: usize = 3;

/// A live graph's publisher together with the runtime mirrors it keeps in
/// step. Cheap to clone; every clone publishes through the same publisher.
#[derive(Clone)]
pub(crate) struct LiveGraph {
    publisher: Arc<Mutex<Publisher>>,
    reclaimer: Arc<Reclaimer>,
    inputs: SyncSender<InputWrite>,
    /// Kept so the audio thread never holds the queue's last handle; the
    /// front doors that submit through it arrive with FUG-310's controls.
    #[cfg_attr(not(test), allow(dead_code))]
    requests: RequestSender,
    state: Arc<Mutex<RuntimeState>>,
    control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
    registry: LiveRegistry,
}

/// What a committed change did, for the caller's follow-up work.
#[derive(Default)]
pub(crate) struct Committed {
    /// Handles of the modules the change built, keyed `<module_id>.<name>`.
    pub(crate) handles: HashMap<String, Arc<dyn Any + Send + Sync>>,
    /// Modules the change built (added or replaced), in build order.
    pub(crate) started: Vec<RuntimeModuleInfo>,
    /// Modules the change removed or replaced.
    pub(crate) stopped: Vec<String>,
}

impl LiveGraph {
    /// Links `graph`, which is about to move to the audio thread, to a new
    /// publisher keeping the given runtime mirrors, with edits building
    /// modules against `registry` until a commit adopts another.
    pub(crate) fn link(
        graph: &mut SignalGraph,
        state: Arc<Mutex<RuntimeState>>,
        control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
        module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
        registry: Arc<ModuleRegistry>,
    ) -> Self {
        let (publisher, ends) = Publisher::link(graph);
        Self {
            publisher: Arc::new(Mutex::new(publisher)),
            reclaimer: Arc::new(Reclaimer::new(ends.retired)),
            inputs: ends.inputs,
            requests: ends.requests,
            state,
            control_surfaces,
            module_ports,
            registry: LiveRegistry::new(registry),
        }
    }

    /// The registry edits build modules against now: the latest one a
    /// commit adopted (see [`Self::commit_adopting`]).
    pub(crate) fn registry(&self) -> Arc<ModuleRegistry> {
        self.registry.current()
    }

    /// Starts freeing retired publications on a control thread every
    /// [`reclaim::RECLAIM_INTERVAL`], so a removed module (a sink finalizing
    /// its file, say) is dropped promptly rather than at the next change.
    /// The thread ends once every clone of this graph is gone. Returns false
    /// when no thread could start.
    pub(crate) fn start_reclaimer(&self) -> bool {
        Reclaimer::spawn(&self.reclaimer)
    }

    /// Frees retired publications now, on the calling thread.
    pub(crate) fn reclaim(&self) -> usize {
        self.reclaimer.reclaim()
    }

    /// Publications made so far. A change prepared from state read before
    /// this generation moved is refused when it publishes.
    pub(crate) fn generation(&self) -> u64 {
        self.publisher.lock().unwrap().generation()
    }

    /// The shared publisher, for observation in tests.
    #[cfg(test)]
    pub(crate) fn publisher(&self) -> &Arc<Mutex<Publisher>> {
        &self.publisher
    }

    /// Starts preparing a change against the current topology, first freeing
    /// whatever the audio thread has retired. The change is stale once any
    /// other change publishes; [`Self::edit`] rules that out for a change
    /// small enough to prepare under the publisher's lock.
    pub(crate) fn begin(&self) -> GraphChange {
        self.reclaim();
        let publisher = self.publisher.lock().unwrap();
        self.change_on(&publisher)
    }

    fn change_on(&self, publisher: &Publisher) -> GraphChange {
        GraphChange::new(
            publisher.generation(),
            publisher.mirror().clone(),
            self.control_surfaces.clone(),
            publisher.block_size(),
        )
    }

    /// Publishes a prepared change and commits the runtime mirrors together.
    /// Fails with nothing changed when another change published since
    /// [`Self::begin`] ([`GraphCommandError::TopologyMoved`]), an empty
    /// change included, or the audio thread is gone. An empty change that
    /// is still current publishes nothing.
    // Live callers retain state with the change through `commit_with`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn commit(&self, prepared: PreparedChange) -> Result<Committed, GraphCommandError> {
        self.commit_with(prepared, |_| {})
    }

    /// [`Self::commit`], then `retain` on the runtime state in the same step,
    /// before the publisher is released: whatever `retain` records (a
    /// reload's document, say) lands with the change it describes, and no
    /// other edit can commit in between. `retain` runs only when the change
    /// commits, an empty change included.
    pub(crate) fn commit_with(
        &self,
        prepared: PreparedChange,
        retain: impl FnOnce(&mut RuntimeState),
    ) -> Result<Committed, GraphCommandError> {
        self.commit_locked(self.publisher.lock().unwrap(), prepared, retain)
    }

    /// [`Self::commit_with`], also adopting `registry`, when given, as the
    /// registry later edits build against, in the same step. An edit that
    /// built a module against the previous registry and has yet to commit is
    /// refused and builds it again (see [`Self::add_module`]), so no edit
    /// commits an instance of a superseded definition. Adopted only when the
    /// change commits, an empty change included.
    pub(crate) fn commit_adopting(
        &self,
        prepared: PreparedChange,
        registry: Option<Arc<ModuleRegistry>>,
        retain: impl FnOnce(&mut RuntimeState),
    ) -> Result<Committed, GraphCommandError> {
        let mut superseded = None;
        let publisher = self.publisher.lock().unwrap();
        // `retain` runs only when the change commits, under the publisher.
        let result = self.commit_locked(publisher, prepared, |state| {
            superseded = registry.map(|registry| self.registry.replace(registry));
            retain(state);
        });
        // The previous registry drops off the publisher's lock.
        drop(superseded);
        result
    }

    /// Prepares and publishes one change while holding the publisher, so no
    /// other change can publish in between and it never goes stale. `edit`
    /// applies the change's edits; build modules before calling, so builds
    /// stay outside the lock.
    pub(crate) fn edit(
        &self,
        edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
    ) -> Result<Committed, GraphCommandError> {
        self.edit_against(None, edit)
    }

    /// [`Self::edit`] for an edit adding modules built against `built_with`:
    /// refused with [`GraphCommandError::TopologyMoved`], before `edit`
    /// runs, when a commit has adopted another registry since.
    fn edit_against(
        &self,
        built_with: Option<&Arc<ModuleRegistry>>,
        edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
    ) -> Result<Committed, GraphCommandError> {
        self.reclaim();
        let publisher = self.publisher.lock().unwrap();
        if built_with.is_some_and(|registry| !self.registry.is_current(registry)) {
            drop(publisher);
            return Err(GraphCommandError::TopologyMoved);
        }
        let mut change = self.change_on(&publisher);
        if let Err(error) = edit(&mut change).and_then(|()| change.attach()) {
            // The change's instances drop off the lock.
            drop(publisher);
            drop(change);
            return Err(error);
        }
        let discarded = change.take_discarded();
        let result = self.commit_locked(publisher, change.compile(), |_| {});
        // Modules the edit displaced drop off the lock too.
        drop(discarded);
        result
    }

    /// Publishes `prepared` and commits the mirrors under `publisher`. A
    /// stale change is refused before the empty-change shortcut, so a caller
    /// never commits work planned against a topology that has moved.
    /// Whatever the change or the publication leaves to free is dropped
    /// after the guard is released.
    fn commit_locked(
        &self,
        mut publisher: MutexGuard<'_, Publisher>,
        prepared: PreparedChange,
        retain: impl FnOnce(&mut RuntimeState),
    ) -> Result<Committed, GraphCommandError> {
        if prepared.base_generation != publisher.generation() {
            drop(publisher);
            drop(prepared);
            return Err(GraphCommandError::TopologyMoved);
        }
        if prepared.is_empty() {
            retain(&mut self.state.lock().unwrap());
            drop(publisher);
            return Ok(Committed::default());
        }
        let mut published = match publisher.publish(prepared) {
            Ok(published) => published,
            Err(Refused { error, change }) => {
                drop(publisher);
                drop(change);
                return Err(error);
            }
        };
        let superseded = published.superseded.take();
        let mirror = publisher.mirror();
        let removed: Vec<&String> = published
            .previous
            .modules
            .keys()
            .filter(|id| !mirror.modules.contains_key(*id))
            .collect();

        {
            let mut surfaces = self.control_surfaces.lock().unwrap();
            for id in &removed {
                surfaces.shift_remove(*id);
            }
            for (id, module) in &published.built {
                match &module.surface {
                    Some(surface) => {
                        surfaces.insert(id.clone(), surface.clone());
                    }
                    None => {
                        surfaces.shift_remove(id);
                    }
                }
            }
        }
        *self.module_ports.lock().unwrap() = mirror
            .modules
            .iter()
            .map(|(id, module)| (id.clone(), module.ports.clone()))
            .collect();
        {
            let mut state = self.state.lock().unwrap();
            for id in &removed {
                state.modules.shift_remove(*id);
                state.document_remove_module(id);
            }
            for (id, module) in &published.built {
                state.modules.insert(id.clone(), module.info.clone());
                state.document_upsert_module(id, &module.info.module_type, &module.info.config);
            }
            state.connections = mirror.edges.iter().map(connection_info).collect();
            retain(&mut state);
        }

        let mut committed = Committed {
            stopped: removed.into_iter().cloned().collect(),
            ..Committed::default()
        };
        for (id, module) in published.built {
            if published.previous.modules.contains_key(&id) {
                committed.stopped.push(id.clone());
            }
            for (name, handle) in module.handles {
                committed.handles.insert(format!("{id}.{name}"), handle);
            }
            committed.started.push(module.info);
        }
        drop(publisher);
        drop(superseded);
        Ok(committed)
    }

    /// Queues a direct write to a module's input port for the next block.
    ///
    /// The module and port are resolved here, against the publisher's
    /// mirror, so an unknown module or port fails at once with
    /// [`GraphCommandError::UnknownModule`] or
    /// [`GraphCommandError::InvalidPort`]. Fails with
    /// [`GraphCommandError::QueueFull`] when the audio thread has not drained
    /// earlier writes, and with [`GraphCommandError::AudioThreadStopped`] when
    /// it is gone.
    ///
    /// Takes the publisher briefly, so a write may wait for an [`Self::edit`]
    /// to attach and compile its change. The write is queued before the
    /// publisher is released, so it reaches the audio thread before any
    /// publication made after it (see `graph::publication::AudioLink`).
    pub(crate) fn write_input(
        &self,
        module_id: &str,
        port: &str,
        value: f32,
    ) -> Result<(), GraphCommandError> {
        let mut publisher = self.publisher.lock().unwrap();
        let write = publisher.input_write(module_id, port, value)?;
        let sent = self.inputs.try_send(write);
        if sent.is_ok() {
            publisher.note_written();
        }
        drop(publisher);
        sent.map_err(|error| match error {
            TrySendError::Full(_) => GraphCommandError::QueueFull,
            TrySendError::Disconnected(_) => GraphCommandError::AudioThreadStopped,
        })
    }

    /// Adds a module, replacing one with the same id in place. Built
    /// against the live graph's current registry (see [`Self::build_then`]).
    pub(crate) fn add_module(
        &self,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<Committed, GraphCommandError> {
        self.build_then(sample_rate, id, module_type, config, |_| Ok(()))
    }

    /// Replaces an existing module. Compatible connections survive when
    /// `preserve_connections` is true; otherwise all of its connections go.
    pub(crate) fn swap_module(
        &self,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
        preserve_connections: bool,
    ) -> Result<Committed, GraphCommandError> {
        self.build_then(sample_rate, id, module_type, config, |change| {
            if !change.contains(id) {
                return Err(GraphCommandError::UnknownModule(id.to_string()));
            }
            if !preserve_connections {
                change.disconnect_module(id);
            }
            Ok(())
        })
    }

    /// Builds module `id` against the current registry, off the publisher's
    /// lock, then under the lock runs `edit` and upserts the module. When a
    /// commit adopts another registry in between (a reload that changed a
    /// development's definition, say), the module is dropped and built again
    /// against the new one, up to [`BUILD_ATTEMPTS`] times in all, after
    /// which the edit fails with [`GraphCommandError::TopologyMoved`].
    fn build_then(
        &self,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
        edit: impl Fn(&mut GraphChange) -> Result<(), GraphCommandError>,
    ) -> Result<Committed, GraphCommandError> {
        let mut attempt = 1;
        loop {
            let registry = self.registry.current();
            let mut built =
                match GraphChange::build(&registry, sample_rate, id, module_type, config) {
                    Ok(built) => Some(built),
                    // A registry adopted since may know the type, or build it.
                    Err(_) if attempt < BUILD_ATTEMPTS && !self.registry.is_current(&registry) => {
                        attempt += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
            // `built` is held out here so a refused edit drops the module
            // after `edit_against` releases the publisher, not inside the
            // closure.
            let result = self.edit_against(Some(&registry), |change| {
                edit(change)?;
                change.upsert(id, built.take().expect("upserted once"));
                Ok(())
            });
            drop(built);
            match result {
                // Under the publisher, only a registry adopted since the
                // build moves the topology.
                Err(GraphCommandError::TopologyMoved) if attempt < BUILD_ATTEMPTS => attempt += 1,
                result => return result,
            }
        }
    }

    /// Removes a module and its connections; a missing module is a no-op.
    pub(crate) fn remove_module(&self, id: &str) -> Result<Committed, GraphCommandError> {
        self.edit(|change| {
            change.remove(id);
            Ok(())
        })
    }

    /// Connects two ports after validating both ends.
    pub(crate) fn connect(&self, edge: RoutingConnection) -> Result<(), GraphCommandError> {
        self.edit(|change| change.connect(edge)).map(drop)
    }

    /// Removes a connection; a missing connection is a no-op.
    pub(crate) fn disconnect(&self, edge: RoutingConnection) -> Result<(), GraphCommandError> {
        self.edit(|change| {
            change.disconnect(edge);
            Ok(())
        })
        .map(drop)
    }
}

/// An edge in the runtime's connection form.
fn connection_info(edge: &RoutingConnection) -> RuntimeConnectionInfo {
    RuntimeConnectionInfo {
        from: edge.from_module.clone(),
        from_port: edge.from_port.clone(),
        to: edge.to_module.clone(),
        to_port: edge.to_port.clone(),
    }
}

/// An edge from string endpoints.
pub(crate) fn edge(from: &str, from_port: &str, to: &str, to_port: &str) -> RoutingConnection {
    RoutingConnection {
        from_module: from.to_string(),
        from_port: from_port.to_string(),
        to_module: to.to_string(),
        to_port: to_port.to_string(),
    }
}
