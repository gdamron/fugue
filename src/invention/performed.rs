//! Which controls were written as performances, for the values a save
//! would not keep (`rpc::DiffSource`). Kept on the control side: a front
//! door marks a control as its performed write is submitted, and a reader
//! asks each control's surface what it sounds now. Nothing here runs on the
//! audio thread.

use super::authored_document::control_json;
use super::orchestration::RuntimeSnapshot;
use crate::rpc::SoundingValue;
use indexmap::IndexSet;

/// The controls written as performances since each module was built, in
/// the order first written. A module built again or removed is forgotten:
/// what it sounds is then what its config builds.
#[derive(Debug, Clone, Default)]
pub(crate) struct PerformedKeys(IndexSet<(String, String)>);

impl PerformedKeys {
    pub(crate) fn mark(&mut self, module_id: &str, key: &str) {
        self.0.insert((module_id.to_string(), key.to_string()));
    }

    pub(crate) fn forget(&mut self, module_id: &str) {
        self.0.retain(|(id, _)| id != module_id);
    }
}

impl RuntimeSnapshot {
    /// Each control written as a performance since its module was built,
    /// with the value it sounds now, read from its surface on the calling
    /// thread. A write a script made is not one: a script plays it again as
    /// it runs. Whether the document holds the value is the reader's to
    /// decide (`rpc::DiffSource`).
    pub fn performed_values(&self) -> Vec<SoundingValue> {
        let keys: Vec<(String, String)> = {
            let state = self.state.lock().unwrap();
            state.performed.0.iter().cloned().collect()
        };
        let surfaces = self.control_surfaces.lock().unwrap();
        (keys.into_iter())
            .filter_map(|(module_id, key)| {
                let value = surfaces.get(&module_id)?.get_control(&key).ok()?;
                Some(SoundingValue {
                    value: control_json(&value),
                    module_id,
                    key,
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "performed_tests.rs"]
mod tests;
