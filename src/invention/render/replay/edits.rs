//! A replay's timed edits: each one that landed live, with its modules
//! built ahead, landed on its sample as one offline change.

use super::super::RenderEngine;
use crate::invention::publish::{edge, BuiltModule, GraphChange};
use crate::{LoggedConnection, ModuleRegistry, RequestLog};

/// A timed edit that landed live, due to land again.
#[derive(Default)]
pub(in crate::invention::render) struct DueEdit {
    /// The sample it landed on.
    pub(super) sample: u64,
    removed: Vec<String>,
    /// Its modules, built when the replay began, in order.
    built: Vec<(String, BuiltModule)>,
    connections: Vec<LoggedConnection>,
}

/// The timed edits in `log` that landed, their modules built from
/// `registry` and set as the batch that built them set them. Fails naming the first out of time order, or the first
/// module that does not build.
pub(super) fn build(
    log: &RequestLog,
    registry: &ModuleRegistry,
    sample_rate: u32,
) -> Result<Vec<DueEdit>, String> {
    let mut edits: Vec<DueEdit> = Vec::new();
    for (n, edit) in log.timed_edits.iter().enumerate() {
        let Some(sample) = edit.landed() else {
            continue;
        };
        if edits.last().is_some_and(|last| last.sample > sample) {
            return Err(format!("logged timed edit {} is out of time order", n + 1));
        }
        let built = edit
            .built
            .iter()
            .map(|module| {
                let config = &module.config;
                GraphChange::build(
                    registry,
                    sample_rate,
                    &module.id,
                    &module.module_type,
                    config,
                )
                .map(|built| {
                    // As the batch made them, failures included.
                    if let Some(surface) = &built.surface {
                        for setting in &module.writes {
                            let _ = surface.set_control(&setting.key, setting.value.clone());
                        }
                    }
                    (module.id.clone(), built)
                })
                .map_err(|error| {
                    format!(
                        "logged timed edit {} cannot build its module: {error}",
                        n + 1
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        edits.push(DueEdit {
            sample,
            removed: edit.removed.clone(),
            built,
            connections: edit.connections.clone(),
        });
    }
    Ok(edits)
}

impl RenderEngine {
    /// Lands `edit` on the render's graph as one offline change, as a timed
    /// edit lands live (what it displaces plays first; see
    /// `SignalGraph::install_offline`): the
    /// modules it removed go, those it built replace theirs in place or
    /// join after the rest, and the connections are then exactly those it
    /// left, in order. Afterwards the requests before the next edit
    /// resolve against the graph it leaves. One the render refuses (a
    /// schedule that no longer resolves, a port missing) is counted
    /// refused, as a request is.
    pub(in crate::invention::render) fn land_replayed(&mut self, edit: DueEdit) {
        let landed = self.loaded().cloned().and_then(|graph| {
            self.edit_landing(&graph, true, |change| {
                change.disconnect_all();
                for id in &edit.removed {
                    change.remove(id);
                }
                for (id, module) in edit.built {
                    change.upsert(&id, module);
                }
                for c in &edit.connections {
                    change.connect(edge(&c.from, &c.from_port, &c.to, &c.to_port))?;
                }
                Ok(())
            })
        });
        match landed {
            Ok(committed) => self.follow_up(committed),
            Err(_) => self.replay.refused += 1,
        }
        let surfaces = self.control_surfaces.lock().unwrap();
        self.replay.landed(&surfaces);
    }
}
