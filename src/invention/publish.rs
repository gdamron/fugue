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
//!    connections, ports, control surfaces, document) update together. After
//!    the publication is queued nothing can fail.
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
use crate::ModuleRegistry;

mod change;
mod publisher;
mod reclaim;
#[cfg(test)]
mod tests;

pub(crate) use change::{GraphChange, PreparedChange};
pub(crate) use publisher::Publisher;
pub(crate) use reclaim::Reclaimer;

/// A live graph's publisher together with the runtime mirrors it keeps in
/// step. Cheap to clone; every clone publishes through the same publisher.
#[derive(Clone)]
pub(crate) struct LiveGraph {
    publisher: Arc<Mutex<Publisher>>,
    reclaimer: Arc<Reclaimer>,
    inputs: SyncSender<InputWrite>,
    state: Arc<Mutex<RuntimeState>>,
    control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
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
    /// publisher keeping the given runtime mirrors.
    pub(crate) fn link(
        graph: &mut SignalGraph,
        state: Arc<Mutex<RuntimeState>>,
        control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
        module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
    ) -> Self {
        let (publisher, ends) = Publisher::link(graph);
        Self {
            publisher: Arc::new(Mutex::new(publisher)),
            reclaimer: Arc::new(Reclaimer::new(ends.retired)),
            inputs: ends.inputs,
            state,
            control_surfaces,
            module_ports,
        }
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
    /// [`Self::begin`] ([`GraphCommandError::TopologyMoved`]) or the audio
    /// thread is gone. An empty change publishes nothing.
    pub(crate) fn commit(&self, prepared: PreparedChange) -> Result<Committed, GraphCommandError> {
        if prepared.is_empty() {
            return Ok(Committed::default());
        }
        self.commit_locked(self.publisher.lock().unwrap(), prepared)
    }

    /// Prepares and publishes one change while holding the publisher, so no
    /// other change can publish in between and it never goes stale. `edit`
    /// applies the change's edits; build modules before calling, so builds
    /// stay outside the lock.
    pub(crate) fn edit(
        &self,
        edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
    ) -> Result<Committed, GraphCommandError> {
        self.reclaim();
        let publisher = self.publisher.lock().unwrap();
        let mut change = self.change_on(&publisher);
        edit(&mut change)?;
        let prepared = change.prepare()?;
        if prepared.is_empty() {
            return Ok(Committed::default());
        }
        self.commit_locked(publisher, prepared)
    }

    fn commit_locked(
        &self,
        mut publisher: MutexGuard<'_, Publisher>,
        prepared: PreparedChange,
    ) -> Result<Committed, GraphCommandError> {
        let published = publisher.publish(prepared)?;
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
        Ok(committed)
    }

    /// Queues a direct write to a module's input port for the next block.
    /// Fails with [`GraphCommandError::QueueFull`] when the audio thread
    /// has not drained earlier writes, and with
    /// [`GraphCommandError::AudioThreadStopped`] when it is gone. Never waits on
    /// the publisher, so a write is not held up by a change being prepared.
    pub(crate) fn write_input(&self, write: InputWrite) -> Result<(), GraphCommandError> {
        self.inputs.try_send(write).map_err(|error| match error {
            TrySendError::Full(_) => GraphCommandError::QueueFull,
            TrySendError::Disconnected(_) => GraphCommandError::AudioThreadStopped,
        })
    }

    /// Adds a module, replacing one with the same id in place.
    pub(crate) fn add_module(
        &self,
        registry: &ModuleRegistry,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<Committed, GraphCommandError> {
        let built = GraphChange::build(registry, sample_rate, id, module_type, config)?;
        self.edit(|change| {
            change.upsert(id, built);
            Ok(())
        })
    }

    /// Replaces an existing module. Compatible connections survive when
    /// `preserve_connections` is true; otherwise all of its connections go.
    pub(crate) fn swap_module(
        &self,
        registry: &ModuleRegistry,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
        preserve_connections: bool,
    ) -> Result<Committed, GraphCommandError> {
        let built = GraphChange::build(registry, sample_rate, id, module_type, config)?;
        self.edit(|change| {
            if !change.contains(id) {
                return Err(GraphCommandError::UnknownModule(id.to_string()));
            }
            if !preserve_connections {
                change.disconnect_module(id);
            }
            change.upsert(id, built);
            Ok(())
        })
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
