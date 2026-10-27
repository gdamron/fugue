//! A commit's writes to the modules it kept that a timed authored write
//! contests: one waiting as the commit lands, or applied and not yet
//! recorded. Either may apply before the commit's own write to the same
//! control, so that write is recorded as it applies, as the value the
//! commit's document holds, and the document ends on whichever applied
//! later, as playback does (see `publish::timed_write`).
//!
//! What each records is read under the publisher, as the commit lands,
//! so no timed write recorded after it can be taken for the commit's.

use std::sync::PoisonError;

use super::{Authoring, LiveGraph};
use crate::invention::authored_document;
use crate::invention::reload::scalar_control_value;

/// The contested writes of a commit, each with what it records: `None`
/// for a control the document holds a whole value (a melody's scale) or
/// nothing for, written unrecorded all the same.
#[derive(Debug, Default)]
pub(crate) struct Contested(Vec<((String, String), Option<Authoring>)>);

impl Contested {
    /// What the commit's write to control `key` of `module_id` records as
    /// it applies; `None` when it is uncontested or records nothing.
    pub(crate) fn authoring(&self, module_id: &str, key: &str) -> Option<&Authoring> {
        let write = self.0.iter().find(|((m, k), _)| m == module_id && k == key);
        write.and_then(|(_, authoring)| authoring.as_ref())
    }
}

impl LiveGraph {
    /// The commit's `writes` to modules it kept, but those it `stopped`,
    /// that a timed authored write contests, read under the publisher once
    /// the commit has recorded its document.
    pub(crate) fn contested(&self, stopped: &[String], writes: &[(String, String)]) -> Contested {
        // The log first, then the state: never both at once.
        let modules =
            (self.pending.lock().unwrap_or_else(PoisonError::into_inner)).timed_authored_modules();
        let contested = |(module_id, _): &&(String, String)| {
            modules.contains(module_id) && !stopped.contains(module_id)
        };
        let state = self.state.lock().unwrap();
        let document = state.document.as_ref();
        let held = |(module_id, key): &(String, String)| {
            let document = document?;
            let held = authored_document::held_control(document, module_id, key)?;
            Some(Authoring {
                value: scalar_control_value(&held)?,
                origin: document.source_path.clone(),
                safe: None,
                then: Vec::new(),
                timed: false,
                chained: false,
            })
        };
        let written = writes.iter().filter(contested);
        Contested(written.map(|write| (write.clone(), held(write))).collect())
    }
}
