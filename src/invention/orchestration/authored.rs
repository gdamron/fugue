//! Making an authored control write, which the retained document records
//! once it applies (see `publish::authored`).

use std::path::PathBuf;

use super::RuntimeSnapshot;
use crate::invention::audio_assets;
use crate::invention::declared::{Queued, WriteOptions};
use crate::invention::runtime::GraphCommandError;
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
}
