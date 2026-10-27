//! Making an authored control write, which the retained document records
//! once it applies (see `publish::authored`).

use std::path::PathBuf;

use super::RuntimeSnapshot;
use crate::invention::declared::{Queued, WriteOptions};
use crate::invention::publish::Authoring;
use crate::invention::reload::scalar_control_value;
use crate::invention::runtime::GraphCommandError;
use crate::invention::{audio_assets, authored_document};
use crate::ControlValue;

impl RuntimeSnapshot {
    /// Makes an authored write (`options` says what to record once it
    /// applies) as [`Self::set_control_transient`] does, saying what it saw
    /// as it was queued when its module declares its controls (see
    /// `declared::Queued`).
    pub(crate) fn make_authored(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, GraphCommandError> {
        self.submit_transient(module_id, key, value, options)
    }

    /// The value a write to `key` hands its module: a relative path written
    /// to an asset control resolved against the invention's directory, as
    /// the loader resolves the document's, or refused when there is none
    /// (see `audio_assets::resolve_asset_write`). The document records the
    /// value as written, so the invention file it was resolved against comes
    /// back too, for recording it (see `publish::Authoring`).
    pub(super) fn asset_to_load(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Result<(ControlValue, Option<PathBuf>), GraphCommandError> {
        if !audio_assets::is_asset_control(key) {
            return Ok((value.clone(), None));
        }
        let source_path = self.state.lock().unwrap().source_path().map(PathBuf::from);
        let load = audio_assets::resolve_asset_write(source_path.as_deref(), key, value)
            .map_err(GraphCommandError::ControlError)?;
        Ok((load, source_path))
    }

    /// Writes `value`, a commit's, to control `key` of `module_id`, kept by
    /// the commit, as [`Self::set_control_transient`] does: unrecorded, the
    /// retained document holding it already. Unless the commit reported the
    /// module `contested`, a timed authored write to it waiting (see
    /// `publish::Committed::contested`): then the write is recorded as it
    /// applies, as the value the document holds, so the document ends on
    /// whichever of the two applied later, as playback does. An event, or
    /// a control the document holds a whole value (a melody's scale) or
    /// nothing for, is written unrecorded all the same.
    pub(crate) fn set_control_committed(
        &self,
        (module_id, key): (&str, &str),
        value: ControlValue,
        contested: &[String],
    ) -> Result<(), GraphCommandError> {
        let contested = contested.iter().any(|module| module == module_id);
        let authoring = contested
            .then(|| self.held_authoring(module_id, key))
            .flatten();
        let Some(authoring) = authoring else {
            return self.set_control_transient(module_id, key, value);
        };
        let options = WriteOptions::default().authored_as(&authoring);
        self.make_authored(module_id, key, value, &options)
            .map(drop)
    }

    /// The scalar the retained document holds for control `key` of
    /// `module_id`, as an authored write records it.
    fn held_authoring(&self, module_id: &str, key: &str) -> Option<Authoring> {
        if self.is_event(module_id, key) {
            return None;
        }
        let state = self.state.lock().unwrap();
        let document = state.document.as_ref()?;
        let held = authored_document::held_control(document, module_id, key)?;
        Some(Authoring {
            value: scalar_control_value(&held)?,
            origin: document.source_path.clone(),
            safe: None,
            then: Vec::new(),
            timed: false,
        })
    }
}
