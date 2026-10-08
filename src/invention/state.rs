use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::invention::authored_document;
use crate::invention::format::{Connection, Invention};
use crate::invention::reload::scalar_control_value;
use crate::modules::AudioDiagnosticsSnapshot;
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Writes a control change into the retained document module's config so
    /// a saved document reproduces the value on a cold rebuild.
    pub(crate) fn document_write_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        if let Some(document) = self.document.as_mut() {
            authored_document::write_control(document, id, key, value);
        }
    }

    /// Records an authored control write: in the retained document, and in
    /// the module's stored config when that config already has the key (see
    /// [`Self::write_stored_control`]).
    ///
    /// Reload plans by diffing the new resolved document against the stored
    /// configs, so a stored config that missed an authored write would make
    /// reloading the original file see no change and keep the written value.
    /// A perform-intent write is never recorded, here or in the document.
    pub(crate) fn record_authored_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        self.document_write_control(id, key, value);
        self.write_stored_control(id, key, value);
    }

    /// Writes an authored control value into a module's stored config only,
    /// for a caller that records the retained document by other means.
    ///
    /// Only a key the stored config already contains is written. A control
    /// key can be absent from it (a default the file omits, an alias such as
    /// a filter's `type` for its `filter_type`, an indexed key such as a
    /// mixer's `level.2` from its `levels` array), and adding one would make
    /// reloading the original file see that key removed, which reload can
    /// only express by rebuilding the module, resetting its phase. Leaving
    /// the key set alone means this can never introduce a rebuild; such a
    /// write stays unrecorded here and survives that reload, as it always
    /// has.
    ///
    /// Only a scalar (a number, bool or string) is overwritten, as only a
    /// scalar delta can reload restore through a control. A stored null,
    /// array or object (an oscillator's `"frequency": null`, a scheduler's
    /// array `schedule`) is left alone, as for an absent key: replacing it
    /// with the written scalar would make reloading the original see a
    /// non-scalar delta, which reload can only express by rebuilding.
    ///
    /// A number is compared in the control's f32 domain: a write the stored
    /// number already holds as an f32 is left as stored, so neither a write
    /// of 440 over an authored `440.0` nor 261.63 over `261.6300048828125`
    /// makes the next reload see a difference in the JSON alone. Does
    /// nothing when no module has `id`.
    pub(crate) fn write_stored_control(&mut self, id: &str, key: &str, value: &ControlValue) {
        let Some(stored) = self
            .modules
            .get_mut(id)
            .and_then(|info| info.config.as_object_mut())
            .and_then(|config| config.get_mut(key))
        else {
            return;
        };
        if scalar_control_value(stored).is_none() {
            return;
        }
        if let (ControlValue::Number(number), Some(previous)) = (value, stored.as_f64()) {
            if previous as f32 == *number {
                return;
            }
        }
        *stored = authored_document::control_json(value);
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
