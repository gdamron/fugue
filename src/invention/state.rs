use std::collections::VecDeque;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::invention::format::{Connection, Invention};
use crate::invention::reload::scalar_control_value;
use crate::invention::{audio_assets, authored_document};
use crate::module_config::{names, replaced_by};
use std::collections::HashMap;

use crate::modules::AudioDiagnosticsSnapshot;
use crate::registry::ReplacingKeys;
use crate::ControlValue;

/// Serializable description of a module in a running invention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeModuleInfo {
    /// Stable instance id inside the graph.
    pub id: String,
    /// Registered module type used to build this instance.
    pub module_type: String,
    /// The config the module was built from (assets resolved), plus later
    /// authored control writes to keys it contains and reload control
    /// updates.
    pub config: serde_json::Value,
}

/// Serializable description of a routed connection in a running invention.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeConnectionInfo {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
}

/// Lightweight runtime status used by orchestration and external APIs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeStatus {
    pub running: bool,
    pub sample_rate: u32,
    pub module_count: usize,
    pub connection_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<AudioDiagnosticsSnapshot>,
    /// Where saving the daemon session stands. Filled in by a daemon; `None`
    /// from the runtime itself, which saves nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::rpc::SessionDurability>,
}

/// Authoritative runtime-owned snapshot of modules, connections, and status.
#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    pub modules: IndexMap<String, RuntimeModuleInfo>,
    pub connections: Vec<RuntimeConnectionInfo>,
    pub sample_rate: u32,
    pub running: bool,
    /// The authored declarative document this graph was built from, kept in
    /// sync with runtime mutations so the live graph can be written back to
    /// disk without losing developments, assets, title, or the
    /// inputs/outputs/controls sections. `None` for graphs assembled without
    /// a document. Module configs stay as authored (`$asset` references
    /// unresolved); connections are mirrored from `connections` on assembly.
    pub document: Option<Invention>,
    /// The lockfile that governed package resolution as this graph's
    /// document was built or last reloaded (see [`crate::pkg::lock`]), set
    /// as the graph commits, a timed reload's as it is adopted. A reload
    /// under another policy rebuilds what may hold package content.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) built_under: crate::pkg::lock::Governance,
    /// The config keys that replace others, per type, as the registry the
    /// graph was built or last reloaded against declares them, for the
    /// writes recorded here (see [`Self::write_stored_control`]).
    pub(crate) replacing: ReplacingKeys,
    /// Which controls load the audio they name, as the same registry
    /// declares them, for the asset writes recorded here (see
    /// [`Self::names_audio`]).
    pub(crate) audio: audio_assets::AudioControls,
    /// The authored writes made to each module since its stored config was
    /// last taken from a document that its stored config tracks apart from
    /// its keys (see [`WrittenSince`]).
    pub(crate) written_since: WrittenSince,
    /// How many authored control writes the document has recorded. A
    /// commit planned from the document before the latest records them
    /// again on top of its own, or, when they are no longer all in
    /// `authored_log`, is refused and planned again (see
    /// `PreparedChange::authored_at`), so the writes it was planned without
    /// are not overwritten.
    pub(crate) authored_epoch: u64,
    pub(crate) authored_log: AuthoredLog,
}

/// What each module's stored config does not show of the authored writes
/// made to it since the config was last taken from a document (see
/// [`RuntimeState::write_stored_control`]). Forgotten for a module as it is
/// built from a document's config, and for all as a reload adopts one.
#[derive(Debug, Clone, Default)]
pub(crate) struct WrittenSince {
    /// The whole values (keys that replace others) written: within one, a
    /// later write is recorded as the module plays it.
    wholes: HashMap<String, Vec<&'static str>>,
    /// The controls written that the config does not record: a key it
    /// leaves out (a default, an indexed key) or holds as a null, array or
    /// object, or one within a whole value the document holds. A reload
    /// keeping the module restores each the new config does not state as a
    /// scalar to its value in a fresh build of that config.
    unrecorded: HashMap<String, Vec<String>>,
}

impl WrittenSince {
    /// Forgets every module's writes: each config is a document's now.
    pub(crate) fn clear(&mut self) {
        self.wholes.clear();
        self.unrecorded.clear();
    }

    /// Forgets the writes to module `id`, built or removed.
    pub(crate) fn forget(&mut self, id: &str) {
        self.wholes.remove(id);
        self.unrecorded.remove(id);
    }

    /// The controls of `id` written that its stored config does not record.
    pub(crate) fn unrecorded(&self, id: &str) -> &[String] {
        self.unrecorded.get(id).map_or(&[], Vec::as_slice)
    }

    fn mark_unrecorded(&mut self, id: &str, key: &str) {
        let keys = self.unrecorded.entry(id.to_string()).or_default();
        if !keys.iter().any(|written| written == key) {
            keys.push(key.to_string());
        }
    }
}

/// The latest authored control writes the document recorded, oldest first.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuthoredLog(VecDeque<(String, String, ControlValue)>);

impl AuthoredLog {
    /// How many it keeps: far more than front doors record while a batch
    /// or reload is planned and built, even one loading samples. Beyond it,
    /// the commit is planned again; each kept write holds its value, a
    /// schedule's text among them.
    const CAPACITY: usize = 64;

    fn push(&mut self, write: (String, String, ControlValue)) {
        if self.0.len() == Self::CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back(write);
    }

    /// The last `count` recorded, oldest first; `None` when it no longer
    /// keeps them all.
    pub(crate) fn last(&self, count: u64) -> Option<Vec<(String, String, ControlValue)>> {
        let count = usize::try_from(count)
            .ok()
            .filter(|&count| count <= self.0.len())?;
        Some(self.0.iter().skip(self.0.len() - count).cloned().collect())
    }
}

impl RuntimeState {
    /// Builds a summary view suitable for tooling and scripting APIs.
    pub fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            running: self.running,
            sample_rate: self.sample_rate,
            module_count: self.modules.len(),
            connection_count: self.connections.len(),
            diagnostics: None,
            session: None,
        }
    }

    /// Records a module added, replaced, or swapped at runtime in the
    /// retained document.
    pub(crate) fn document_upsert_module(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) {
        if let Some(document) = self.document.as_mut() {
            authored_document::upsert_module(document, id, module_type, config);
        }
    }

    /// Removes a module from the retained document.
    pub(crate) fn document_remove_module(&mut self, id: &str) {
        if let Some(document) = self.document.as_mut() {
            authored_document::remove_module(document, id);
        }
    }

    /// Whether the retained document holds `value` for control `key` of
    /// `id`, as a scalar.
    pub(crate) fn holds(&self, id: &str, key: &str, value: &ControlValue) -> bool {
        let held =
            (self.document.as_ref()).and_then(|d| authored_document::held_control(d, id, key));
        held.and_then(|held| scalar_control_value(&held)).as_ref() == Some(value)
    }

    /// Writes a control change into the retained document module's config so
    /// a saved document reproduces the value on a cold rebuild.
    pub(crate) fn document_write_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        let replacing = &self.replacing;
        if let Some(document) = self.document.as_mut() {
            authored_document::write_control(document, id, key, value, |module_type| {
                replacing.config_keys(module_type)
            });
        }
    }

    /// Records an authored control write: in the retained document, and in
    /// the module's stored config when that config already has the key (see
    /// [`Self::write_stored_control`]).
    ///
    /// Reload plans by diffing the new resolved document against the stored
    /// configs, so a stored config that missed an authored write would make
    /// reloading the original file see no change and keep the written value;
    /// a write it cannot hold is noted apart for reload to restore.
    /// A perform-intent write is never recorded, here or in the document.
    /// Whether control `key` of module `id` loads the audio it names, as
    /// the module's declaration has it: an `asset` a development exposes
    /// from a frequency does not, and its value is no path.
    pub(crate) fn names_audio(&self, id: &str, key: &str) -> bool {
        match self.modules.get(id) {
            Some(info) => self.audio.names_audio(&info.module_type, key),
            None => audio_assets::is_asset_control(key),
        }
    }

    pub(crate) fn record_authored_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        self.authored_epoch += 1;
        (self.authored_log).push((id.to_string(), key.to_string(), value.clone()));
        self.document_write_control(id, key, value);
        self.write_stored_control(id, key, value);
    }

    /// Writes back control `key` of `id` after a write to it failed as it
    /// was made, leaving the module playing what it did.
    ///
    /// The document takes back what `before`, the document as it was before
    /// the failing commit, held for the control, or holds nothing for it,
    /// never the module's value, which a performed write may have set. A
    /// relative asset path is rebased from `before`'s directory. The stored
    /// config takes the scalar `before` held, or else the module's `actual`
    /// value, so a later reload diffs against what plays and tries the
    /// write again. Neither is a new authored write, so nothing is logged.
    pub(crate) fn restore_authored_control(
        &mut self,
        before: Option<&Invention>,
        (id, key): (&str, &str),
        actual: &ControlValue,
    ) {
        let held = before.and_then(|before| {
            let held = authored_document::held_control(before, id, key)?;
            let Some(scalar) = scalar_control_value(&held) else {
                return Some(held);
            };
            let from = before.source_path.as_deref();
            let asset = self.names_audio(id, key);
            let rebased =
                audio_assets::rebase_asset_write(from, self.source_path(), asset, &scalar);
            Some(match rebased == scalar {
                true => held,
                false => authored_document::control_json(&rebased),
            })
        });
        let stored = held.as_ref().and_then(scalar_control_value);
        self.write_stored_control(id, key, stored.as_ref().unwrap_or(actual));
        if let Some(document) = self.document.as_mut() {
            authored_document::restore_control(document, id, key, held);
        }
    }

    /// Writes an authored control value into a module's stored config only,
    /// for a caller that records the retained document by other means.
    ///
    /// Only a key the stored config already contains is written. A control
    /// key can be absent from it (a default the file omits, an indexed key
    /// such as a mixer's `level.2` from its `levels` array), and adding one
    /// would make reloading the original file see that key removed, which
    /// reload can only express by rebuilding the module, resetting its phase.
    /// Leaving the key set alone means this can never introduce a rebuild;
    /// such a write is noted as unrecorded instead ([`WrittenSince`]), and
    /// reloading the original restores it by a control write.
    ///
    /// Only a scalar (a number, bool or string) is overwritten, as only a
    /// scalar delta can reload restore through a control. A stored null,
    /// array or object (an oscillator's `"frequency": null`, a scheduler's
    /// array `schedule`) is left alone and the write noted, as for an
    /// absent key: replacing it with the written scalar would make reloading
    /// the original see a non-scalar delta, which reload can only express by
    /// rebuilding.
    ///
    /// A number is compared in the control's f32 domain: a write the stored
    /// number already holds as an f32 is left as stored, so neither a write
    /// of 440 over an authored `440.0` nor 261.63 over `261.6300048828125`
    /// makes the next reload see a difference in the JSON alone.
    ///
    /// A sample's `asset.N` is written into the entry it names, as the
    /// document records it (see `authored_document::entry_asset_mut`), so a
    /// reload of the saved document sees no change and keeps the module,
    /// while a reload of a file naming another sample rebuilds it to play
    /// that one. An asset's package ref is stored resolved, as the load
    /// resolved it (see `audio_assets::stored_asset_write`), and so is a
    /// relative asset path, against the invention's directory (see
    /// `audio_assets::resolve_asset_write`): the next reload compares the
    /// file a write names, not its spelling.
    ///
    /// A write replacing other keys (a melody's whole scale, see
    /// [`ConfigKey::replacing`](crate::module_config::ConfigKey::replacing)) rewrites what the module
    /// plays wholesale, so the stored config forgets the keys it replaced
    /// and records it, whatever was stored; until the config is next taken
    /// from a document, a later write to one of those keys is recorded too,
    /// stored or not, as the module plays it within that value. Within a
    /// whole value the document holds, such a write is recorded only when
    /// stored, as any other is. Reloading the document saved
    /// after it then changes nothing, and reloading the original brings
    /// back what the write discarded: by control writes, or by a rebuild
    /// when the original's value is not one a control takes (a scale
    /// written as an array). Does nothing when no module has `id`.
    pub(crate) fn write_stored_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        let asset = self.names_audio(id, key);
        let value =
            &audio_assets::resolve_checked_asset_write(self.source_path(), (key, asset), value);
        let Some(info) = self.modules.get_mut(id) else {
            return;
        };
        let keys = self.replacing.config_keys(&info.module_type);
        // Within a whole value written since the config was taken from a
        // document, a key is played as that value rewrote it, so it is
        // recorded whether or not it was stored. Within one the document
        // holds, it is not: it is noted as unrecorded, as any write to a key
        // the config does not hold is, and a reload restores it.
        let written = (self.written_since.wholes.get(id)).map_or(&[][..], Vec::as_slice);
        let within = keys.iter().any(|whole| {
            written.contains(&whole.key)
                && whole.replaces.iter().any(|replaced| names(replaced, key))
        });
        if let Some(whole) = keys
            .iter()
            .find(|whole| whole.key == key && !whole.replaces.is_empty())
        {
            let written = self.written_since.wholes.entry(id.to_string()).or_default();
            if !written.contains(&whole.key) {
                written.push(whole.key);
            }
        }
        if authored_document::forget_replaced(&mut info.config, replaced_by(keys, key)) || within {
            authored_document::write_config_control(&mut info.config, key, value);
            return;
        }
        let value = &*audio_assets::stored_asset_write(asset, value);
        let asset = authored_document::control_json(value);
        if authored_document::write_entry_asset(&mut info.config, key, &asset) {
            return;
        }
        let Some(stored) = (info.config.as_object_mut())
            .and_then(|config| config.get_mut(key))
            .filter(|stored| scalar_control_value(stored).is_some())
        else {
            self.written_since.mark_unrecorded(id, key);
            return;
        };
        if let (ControlValue::Number(number), Some(previous)) = (value, stored.as_f64()) {
            if previous as f32 == *number {
                return;
            }
        }
        *stored = authored_document::control_json(value);
    }

    /// The file the retained document was loaded from or saved to, if any.
    pub(crate) fn source_path(&self) -> Option<&std::path::Path> {
        self.document.as_ref()?.source_path.as_deref()
    }

    /// Assembles the retained declarative document, mirroring the live
    /// graph's connections. Returns `None` when no document was retained.
    pub fn document(&self) -> Option<Invention> {
        let mut document = self.document.clone()?;
        document.connections = self
            .connections
            .iter()
            .map(|conn| Connection {
                from: conn.from.clone(),
                to: conn.to.clone(),
                from_port: (!conn.from_port.is_empty()).then(|| conn.from_port.clone()),
                to_port: (!conn.to_port.is_empty()).then(|| conn.to_port.clone()),
            })
            .collect();
        Some(document)
    }
}
