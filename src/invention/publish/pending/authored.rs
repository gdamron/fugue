//! What the pending log tells the retained document about authored writes
//! not yet recorded: which controls they write, which a commit replaced or
//! followed, and whether any is still to be recorded.

use super::{Authoring, DocumentStep, PendingLog};
use crate::control_request::RequestId;
use crate::ControlValue;

impl PendingLog {
    /// The control of each authored write not yet recorded, waiting to
    /// apply or listed, but those safe from timed edit `except` and timed
    /// writes (see [`Authoring::timed`]).
    pub(crate) fn authored_keys(&self, except: Option<RequestId>) -> Vec<(String, String)> {
        let watched = |authoring: &Authoring| {
            !authoring.timed && (except.is_none() || authoring.safe != except)
        };
        let waiting = (self.writes.iter())
            .filter(|(_, write)| write.authored.as_ref().is_some_and(watched))
            .map(|(_, write)| (write.module_id.clone(), write.key.clone()));
        let listed = self.steps.iter().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } if watched(authoring) => Some((module_id.clone(), key.clone())),
            _ => None,
        });
        waiting.chain(listed).collect()
    }

    /// Leaves each authored write waiting to `module_id.key` for which
    /// `replaced` holds unrecorded when it applies (see `authored::Fence`).
    pub(crate) fn forget_authored(
        &mut self,
        replaced: impl Fn(&str, &str, &Authoring) -> bool,
        except: Option<RequestId>,
    ) {
        let safe = |authoring: &Authoring| except.is_some() && authoring.safe == except;
        for (_, write) in &mut self.writes {
            let forgotten = (write.authored.as_ref())
                .is_some_and(|a| !safe(a) && replaced(&write.module_id, &write.key, a));
            if forgotten {
                write.authored = None;
            }
        }
        // Listed since the publisher last took them: applied before the
        // commit was recorded, so as replaced.
        self.steps.retain(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } => safe(authoring) || !replaced(module_id, key, authoring),
            DocumentStep::Timed => true,
        });
    }

    /// Adds to each authored write waiting, and each listed and not yet
    /// recorded, what `follows` says a commit wrote after it (see
    /// [`Authoring::then`]), but those safe from timed edit `except`, made
    /// after its writes.
    pub(crate) fn follow_authored(
        &mut self,
        follows: impl Fn(&str, &str) -> Vec<(String, ControlValue)>,
        except: Option<RequestId>,
    ) {
        let followed = |authoring: &Authoring| {
            !authoring.timed && (except.is_none() || authoring.safe != except)
        };
        let waiting = (self.writes.iter_mut()).filter_map(|(_, write)| {
            let authoring = write.authored.as_mut().filter(|a| followed(a))?;
            Some((write.module_id.as_str(), write.key.as_str(), authoring))
        });
        let listed = self.steps.iter_mut().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } if followed(authoring) => Some((module_id.as_str(), key.as_str(), authoring)),
            _ => None,
        });
        for (module_id, key, authoring) in waiting.chain(listed) {
            authoring.then.extend(follows(module_id, key));
        }
    }

    /// Whether an authored write waits to apply, or applied and is not yet
    /// recorded. A timed one waiting is not counted: it may wait long, and
    /// is recorded as it applies, after what is read now.
    pub(crate) fn authoring(&self) -> bool {
        !self.steps.is_empty()
            || (self.writes.iter())
                .any(|(_, write)| write.authored.as_ref().is_some_and(|a| !a.timed))
    }

    /// The modules of the timed authored writes waiting, or applied and
    /// not yet recorded, listed once each.
    pub(crate) fn timed_authored_modules(&self) -> Vec<String> {
        let waiting = (self.writes.iter())
            .filter_map(|(_, write)| Some((&write.module_id, write.authored.as_ref()?)));
        let applied = self.steps.iter().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                authoring,
                ..
            } => Some((module_id, authoring)),
            DocumentStep::Timed => None,
        });
        let mut modules: Vec<String> = Vec::new();
        for (module_id, authoring) in waiting.chain(applied) {
            if authoring.timed && !modules.contains(module_id) {
                modules.push(module_id.clone());
            }
        }
        modules
    }
}
