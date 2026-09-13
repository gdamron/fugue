//! Runtime revision preconditions for safe concurrent document editing.
//!
//! "One daemon, many clients" (see [`identity`]) means an agent can read the
//! authored document, spend a turn reasoning about it, and submit a whole-
//! document edit after another client has already changed the running
//! invention. Without a precondition that edit silently overwrites the newer
//! state.
//!
//! A [`RuntimeRevision`] is the conflict token that closes the window. Every
//! response carries the daemon's current one; a client may echo it back as
//! `RpcRequest::expected_revision` to say "apply this only if nothing has
//! changed since I read." A mismatch is refused with a [`RevisionConflict`]
//! and no mutation at all.
//!
//! # What advances the revision
//!
//! The revision tracks **authoring**, not **performance**. A structural
//! command always advances it; a control write advances it only when the
//! client marks it [`ControlWriteIntent::Author`] — "this is the new starting
//! state". A conducting script or agent writing controls at musical rate uses
//! [`ControlWriteIntent::Perform`], so a continuously running scheduler
//! neither pollutes the retained document nor makes every peer's structural
//! edit permanently stale. [`RpcCommand::advances_revision`] is the single
//! definition of that rule.
//!
//! Deliberately absent: any automatic merge. A rejected client is handed the
//! current revision and re-reads; reconciling is the client's business.
//!
//! [`identity`]: super::identity
//! [`RpcCommand::advances_revision`]: super::RpcCommand::advances_revision

use serde::{Deserialize, Serialize};

use super::RpcCommand;

/// The daemon's current document revision, scoped to the session that minted
/// it.
///
/// `session_id` is the daemon's [`DaemonIdentity::session_id`], so a
/// replacement daemon — which mints a fresh one at startup — invalidates every
/// token issued by its predecessor even if the counters happen to coincide.
/// The counter itself is opaque and monotonic within a session; clients must
/// compare tokens for equality rather than ordering them.
///
/// [`DaemonIdentity::session_id`]: super::DaemonIdentity::session_id
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RuntimeRevision {
    /// The daemon session the revision belongs to.
    pub session_id: String,
    /// Monotonic counter advanced by each authoring change in that session.
    pub revision: u64,
}

/// Why a precondition did not hold.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConflictReason {
    /// Same daemon session, but the document has been authored since the
    /// client read it. Re-read and rebase.
    StaleRevision,
    /// The token was minted by a different daemon session — the daemon was
    /// restarted or replaced. Every pre-restart token is void; re-read.
    SessionReplaced,
}

impl ConflictReason {
    /// Guidance a client can surface verbatim to a human or agent.
    pub fn guidance(self) -> &'static str {
        match self {
            Self::StaleRevision => {
                "the document changed after you read it; re-read and reapply your edit"
            }
            Self::SessionReplaced => "the daemon restarted since you read; re-read before editing",
        }
    }
}

/// The compact structured body of a refused precondition.
///
/// Carries both sides of the comparison so a client can re-read and rebase
/// without a second round trip to discover where it stands. No part of the
/// runtime is mutated when this is returned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RevisionConflict {
    /// The revision the client required.
    pub expected: RuntimeRevision,
    /// The revision the daemon actually holds.
    pub current: RuntimeRevision,
    pub reason: ConflictReason,
}

impl RevisionConflict {
    /// A one-line description suitable for [`RpcError::message`].
    ///
    /// [`RpcError::message`]: super::RpcError::message
    pub fn describe(&self) -> String {
        match self.reason {
            ConflictReason::StaleRevision => format!(
                "stale revision: expected {}, daemon is at {}; {}",
                self.expected.revision,
                self.current.revision,
                self.reason.guidance()
            ),
            ConflictReason::SessionReplaced => format!(
                "daemon session changed: expected {}, daemon is {}; {}",
                self.expected.session_id,
                self.current.session_id,
                self.reason.guidance()
            ),
        }
    }
}

/// Whether a control write sets the invention's new starting state or is a
/// live performance gesture.
///
/// The distinction is what keeps automation from invalidating authoring: a
/// scheduler writing at musical rate performs, it does not author.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlWriteIntent {
    /// The value becomes the module's new starting state: it is recorded in
    /// the retained document (so a save reproduces it) and advances the
    /// revision. The default, so a client that predates this field keeps the
    /// tweak-then-save behavior it was written against.
    #[default]
    Author,
    /// A live gesture. The value is applied and announced as a
    /// `ControlChanged` event, but it does not touch the retained document and
    /// does not advance the revision.
    Perform,
}

impl ControlWriteIntent {
    /// Whether a write with this intent is recorded in the retained document
    /// and advances the revision.
    pub fn is_authoring(self) -> bool {
        matches!(self, Self::Author)
    }
}

/// The daemon's authoring revision counter.
///
/// Owned by the daemon and touched only from its request-handling thread —
/// never from the audio thread, which has no notion of revisions. Advancing is
/// an integer increment; checking is a comparison.
#[derive(Debug, Clone)]
pub struct RevisionTracker {
    session_id: String,
    revision: u64,
}

impl RevisionTracker {
    /// Starts a tracker for a daemon session, at revision 0.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            revision: 0,
        }
    }

    /// The token to stamp on outgoing responses.
    pub fn current(&self) -> RuntimeRevision {
        RuntimeRevision {
            session_id: self.session_id.clone(),
            revision: self.revision,
        }
    }

    /// Records an authoring change, returning the new token.
    pub fn advance(&mut self) -> RuntimeRevision {
        self.revision += 1;
        self.current()
    }

    /// Checks a request's precondition.
    ///
    /// `None` is an unconditional call and always passes — existing clients
    /// that never learned about revisions keep working, and a caller that
    /// genuinely means "apply regardless" simply omits the field.
    pub fn check(&self, expected: Option<&RuntimeRevision>) -> Result<(), RevisionConflict> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let current = self.current();
        if expected == &current {
            return Ok(());
        }
        let reason = if expected.session_id == current.session_id {
            ConflictReason::StaleRevision
        } else {
            ConflictReason::SessionReplaced
        };
        Err(RevisionConflict {
            expected: expected.clone(),
            current,
            reason,
        })
    }
}

impl RpcCommand {
    /// Whether this command is an authoring change, advancing the daemon's
    /// revision whenever it is attempted.
    ///
    /// The single definition of the rule described in the [module docs](self):
    /// structural commands always author; a control write authors only when
    /// its intent says so, which is what keeps a continuously running scheduler
    /// from invalidating every peer's structural edit. Read-only and
    /// runtime-lifecycle commands never advance it.
    pub fn advances_revision(&self) -> bool {
        match self {
            Self::LoadInvention { .. }
            | Self::ReloadInvention { .. }
            | Self::UnloadInvention
            | Self::AddModule { .. }
            | Self::RemoveModule { .. }
            | Self::Connect { .. }
            | Self::Disconnect { .. }
            | Self::SwapModule { .. }
            | Self::LoadExample { .. } => true,
            Self::SetControl { intent, .. } => intent.is_authoring(),
            Self::SetControls { writes } => writes.iter().any(|write| write.intent.is_authoring()),
            Self::SaveInvention { .. }
            | Self::InstallPackage(_)
            | Self::ListPackages
            | Self::ListDevelopments { .. }
            | Self::DescribeDevelopment { .. }
            | Self::ListExamples { .. }
            | Self::DescribeExample { .. }
            | Self::DescribeModuleTypes(_)
            | Self::DescribeModule(_)
            | Self::Shutdown => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(session: &str, revision: u64) -> RuntimeRevision {
        RuntimeRevision {
            session_id: session.to_string(),
            revision,
        }
    }

    #[test]
    fn missing_precondition_is_unconditional() {
        let mut tracker = RevisionTracker::new("s1");
        tracker.advance();
        assert!(tracker.check(None).is_ok());
    }

    #[test]
    fn matching_precondition_passes() {
        let mut tracker = RevisionTracker::new("s1");
        let after = tracker.advance();
        assert!(tracker.check(Some(&after)).is_ok());
    }

    #[test]
    fn stale_revision_conflicts_within_a_session() {
        let mut tracker = RevisionTracker::new("s1");
        let read = tracker.current();
        tracker.advance();
        let conflict = tracker.check(Some(&read)).expect_err("should conflict");
        assert_eq!(conflict.reason, ConflictReason::StaleRevision);
        assert_eq!(conflict.expected, read);
        assert_eq!(conflict.current, token("s1", 1));
    }

    #[test]
    fn a_replacement_session_voids_matching_counters() {
        // Same counter value, different daemon: the token must not be honored.
        let replacement = RevisionTracker::new("s2");
        let conflict = replacement
            .check(Some(&token("s1", 0)))
            .expect_err("should conflict");
        assert_eq!(conflict.reason, ConflictReason::SessionReplaced);
        assert_eq!(conflict.current, token("s2", 0));
    }

    #[test]
    fn checking_never_advances_the_counter() {
        let tracker = RevisionTracker::new("s1");
        let _ = tracker.check(Some(&token("s1", 7)));
        assert_eq!(tracker.current(), token("s1", 0));
    }

    #[test]
    fn author_is_the_default_intent() {
        assert_eq!(ControlWriteIntent::default(), ControlWriteIntent::Author);
        assert!(ControlWriteIntent::Author.is_authoring());
        assert!(!ControlWriteIntent::Perform.is_authoring());
    }
}
