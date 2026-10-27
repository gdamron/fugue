//! `ApplyEdits` timed: the batch checked, validated and prepared now, as
//! an untimed batch is, then queued to land on a sample or a beat, its
//! writes to modules it keeps chained to the landing (see
//! `publish::timed_chain`).

use super::commit::{plan_builds, PreparedEdits};
use super::{Batch, Refused, RELOAD_ATTEMPTS};
use crate::control_request::RequestId;
use crate::invention::audio_assets;
use crate::invention::publish::EditTime;
use crate::invention::runtime::timed::edit_time;
use crate::invention::runtime::{GraphCommandError, RunningInvention};
use crate::invention::state::RuntimeState;
use crate::rpc::{EditWhen, RequestStatus, RpcError, StructuralEdit};
use crate::ControlValue;

impl RunningInvention {
    /// [`Self::apply_edits`], landing `when` rather than at once, unless
    /// `ttl_ms` milliseconds pass first. Returns the queued request's
    /// status. A beat time counts on the clock it names, otherwise on the
    /// invention's first clock.
    ///
    /// Refused as an untimed batch is, and also, with nothing queued, when
    /// it changes no structure (time a control write instead), writes a
    /// control of a module it keeps that cannot be timed (an undeclared
    /// control), adds or removes a code or agent module, or names a time
    /// that can never come.
    ///
    /// Its writes to modules it keeps play from the sample it lands on;
    /// those to modules it adds or rebuilds are made as they are built. Its
    /// document is retained once it is adopted, with any authored write
    /// made meanwhile to a module it keeps and a control it does not write.
    /// While it waits, any other structural edit, reload or edit batch
    /// cancels it, as does an authored write to a module it rebuilds or
    /// removes. It announces no `ControlChanged`: its adoption is the
    /// change.
    pub fn apply_edits_at(
        &mut self,
        edits: &[StructuralEdit],
        when: &EditWhen,
        ttl_ms: Option<u64>,
    ) -> Result<RequestStatus, RpcError> {
        let time = edit_time(when, None)?;
        let ttl = self.ttl_samples(ttl_ms);
        let mut batch = self.live.graph_policy().run(|| self.batch(edits))?;
        let mut attempt = 1;
        loop {
            self.rebatch(edits, &mut batch)?;
            let base = self.live.generation();
            // Built under the running graph's policy, read after `base`.
            let policy = self.live.graph_policy();
            let result = policy
                .run(|| self.prepare_edits(base, &batch))
                .and_then(|prepared| self.queue_edits(prepared, &batch, time, ttl));
            match result {
                Ok(id) => return Ok(self.status_of(id)),
                Err(Refused::Moved) if attempt < RELOAD_ATTEMPTS => attempt += 1,
                Err(Refused::Moved) => return Err(GraphCommandError::TopologyMoved.into()),
                Err(Refused::Batch(error)) => return Err(error),
            }
        }
    }

    /// Queues a prepared batch at `time`, refused after `ttl` samples.
    fn queue_edits(
        &self,
        prepared: PreparedEdits,
        batch: &Batch,
        time: EditTime<'_>,
        ttl: Option<u64>,
    ) -> Result<RequestId, Refused> {
        let PreparedEdits { change, plan } = prepared;
        let kept = |module_id: &str| !plan_builds(&plan, module_id);
        let triple = |module_id: &String, key: &String, value: &ControlValue| {
            (module_id.clone(), key.clone(), value.clone())
        };
        let source_path = batch.candidate.document.source_path.as_deref();
        let writes: Vec<_> = (batch.candidate.final_writes().into_iter())
            .map(|candidate| &candidate.write)
            .filter(|write| kept(&write.module_id))
            .map(|write| {
                let load = audio_assets::resolve_checked_asset_write(
                    source_path,
                    &write.key,
                    &write.value,
                );
                (write.module_id.clone(), write.key.clone(), load)
            })
            .collect();
        // In batch order, so each stored config ends at its last write (see
        // `commit_edits`).
        let stored: Vec<_> = (batch.candidate.control_writes.iter())
            .map(|candidate| &candidate.write)
            .filter(|write| kept(&write.module_id))
            .map(|write| triple(&write.module_id, &write.key, &write.value))
            .collect();
        let document = batch.candidate.document.clone();
        let retain = move |state: &mut RuntimeState| {
            state.document = Some(document);
            for (module_id, key, value) in &stored {
                state.write_stored_control(module_id, key, value);
            }
        };
        let registry = self.registry();
        let built_with = Some(&registry);
        let queued = (self.live).commit_built_when(change, built_with, time, ttl, retain, &writes);
        queued.map_err(|error| match error {
            GraphCommandError::TopologyMoved => Refused::Moved,
            other => Refused::Batch(other.into()),
        })
    }
}
