//! A reload timed: the document validated, planned and prepared now, as an
//! untimed reload is, then queued to land on a sample or a beat, its config
//! changes to modules it keeps chained to the landing (see
//! `publish::timed_chain`).

use super::commit::PreparedCommit;
use super::{ReloadError, ReloadPlan, RELOAD_ATTEMPTS};
use crate::control_request::RequestId;
use crate::invention::declared::DeclaredSurface;
use crate::invention::format::ModuleSpec;
use crate::invention::publish::EditTime;
use crate::invention::runtime::timed::edit_time;
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::invention::state::RuntimeState;
use crate::rpc::{EditWhen, RequestStatus};
use crate::Invention;

impl RunningInvention {
    /// [`Self::reload`], landing `when` rather than at once, unless
    /// `ttl_ms` milliseconds pass first. Returns the queued request's
    /// status. A beat time counts on the clock it names, otherwise on the
    /// invention's first clock.
    ///
    /// Refused as an untimed reload is, and also, with nothing queued, when
    /// the document changes a development's definition (reload it untimed),
    /// changes no structure (time a control write instead), changes a
    /// control of a module it keeps that cannot be timed (an undeclared
    /// control), adds or removes a code or agent module, or names a time
    /// that can never come.
    ///
    /// Its config changes to modules it keeps (a clock's reset included)
    /// play from the sample it lands on. A module it keeps whose change no
    /// condition can time (a payload, such as a melody's whole scale, or a
    /// key removed from an undeclared module) is rebuilt as it lands
    /// instead. Its document is retained once it
    /// is adopted, with any authored write made meanwhile to a module it
    /// keeps and a control it does not change. While it waits, any other
    /// structural edit, reload or edit batch cancels it, as does an
    /// authored write to a module it rebuilds or removes.
    pub fn reload_at(
        &mut self,
        invention: Invention,
        when: &EditWhen,
        ttl_ms: Option<u64>,
    ) -> Result<RequestStatus, ReloadError> {
        let time = edit_time(when, None).map_err(ReloadError::Apply)?;
        let ttl = self.ttl_samples(ttl_ms);
        let validated = self.validate_document(invention)?;
        // Adopting new definitions (and the registry built from them) as a
        // timed edit lands is not supported yet; the current registry
        // already builds the same ones.
        if validated.definitions.loaded != self.development_definitions.loaded {
            let message = "A timed reload cannot change a development's definition yet: \
                           reload untimed, or keep the definitions as they are";
            return Err(ReloadError::Invalid(message.to_string()));
        }
        let mut attempt = 1;
        loop {
            let base = self.live.generation();
            let authored = self.live.authored_epoch();
            let mut plan = self.plan_document(&validated)?;
            self.rebuild_unchained(&mut plan, &validated.resolved);
            let document = Some(validated.document.clone());
            let queued = self
                .prepare_plan(base, plan, document, None)
                .map(|prepared| prepared.planned_at(authored))
                .and_then(|prepared| self.queue_reload(prepared, time, ttl));
            match queued {
                Ok(id) => return Ok(self.status_of(id)),
                Err(GraphCommandError::TopologyMoved) if attempt < RELOAD_ATTEMPTS => attempt += 1,
                Err(error) => return Err(ReloadError::Apply(error)),
            }
        }
    }

    /// Rebuilds, rather than writes as it lands, a module `plan` keeps
    /// whose write no condition can chain to the landing: a payload (a
    /// melody's whole scale), or a key the document removes from a module
    /// whose controls are outside the request queue. Its new spec, built
    /// now, plays that key as a fresh build does from the sample the
    /// reload lands on.
    fn rebuild_unchained(&self, plan: &mut ReloadPlan, document: &Invention) {
        let stated = |config: &serde_json::Value, key: &str| config.get(key).is_some();
        let removed: Vec<(String, String)> = {
            let state = self.state.lock().unwrap();
            (plan.control_updates.iter())
                .filter(|(id, key, _)| {
                    let spec = document.modules.iter().find(|spec| spec.id == *id);
                    let was = state
                        .modules
                        .get(id)
                        .is_some_and(|info| stated(&info.config, key));
                    was && spec.is_some_and(|spec| !stated(&spec.config, key))
                })
                .map(|(id, key, _)| (id.clone(), key.clone()))
                .collect()
        };
        let rebuilt: Vec<ModuleSpec> = {
            let surfaces = self.control_surfaces.lock().unwrap();
            let unchained = |module_id: &str, key: &str| {
                let declared = (surfaces.get(module_id))
                    .and_then(|surface| surface.as_any())
                    .and_then(|any| any.downcast_ref::<DeclaredSurface>());
                match declared {
                    Some(declared) => declared.is_payload(key),
                    None => (removed.iter()).any(|(id, gone)| id == module_id && gone == key),
                }
            };
            (document.modules.iter())
                .filter(|spec| {
                    (plan.control_updates.iter())
                        .any(|(id, key, _)| *id == spec.id && unchained(id, key))
                })
                .cloned()
                .collect()
        };
        for spec in rebuilt {
            plan.control_updates.retain(|(id, _, _)| *id != spec.id);
            plan.refreshed_configs.retain(|(id, _)| *id != spec.id);
            plan.unchanged.retain(|id| *id != spec.id);
            plan.swapped.push(spec);
        }
    }

    fn queue_reload(
        &self,
        prepared: PreparedCommit,
        time: EditTime<'_>,
        ttl: Option<u64>,
    ) -> Result<RequestId, GraphCommandError> {
        let PreparedCommit {
            change,
            document,
            control_updates,
            refreshed_configs,
            #[cfg(not(target_arch = "wasm32"))]
            built_under,
            ..
        } = prepared;
        let retain = move |state: &mut RuntimeState| {
            // Every module's config is the document's now, unchanged or
            // refreshed: none holds a write made since.
            if document.is_some() {
                state.written_since.clear();
            }
            for (module_id, config) in refreshed_configs {
                if let Some(info) = state.modules.get_mut(&module_id) {
                    info.config = config;
                }
            }
            if let Some(document) = document {
                state.document = Some(document);
            }
            // The graph is built under the reload's lock once it lands.
            #[cfg(not(target_arch = "wasm32"))]
            {
                state.built_under = built_under;
            }
        };
        let registry = self.registry();
        let built_with = Some(&registry);
        (self.live).commit_built_when(change, built_with, time, ttl, retain, &control_updates)
    }
}
