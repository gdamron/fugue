//! Mutation recovery after a lost response, a timeout, or a daemon restart.
//!
//! A client that loses its connection between sending an edit and reading the
//! reply cannot tell whether the edit landed. Resending blindly can apply a
//! structural edit twice, or deliver an edit meant for one runtime to the
//! replacement daemon that took its place. This module defines the bounded
//! contract that removes that guesswork.
//!
//! # Tickets
//!
//! A client attaches a [`MutationTicket`] to an authoring command. It carries
//! a client-minted `id` and `issued_at`, the latest [`RuntimeRevision`] the
//! client had seen when it minted the ticket. On a retry the client resends
//! the command with the **same** ticket. The daemon answers from its
//! [`MutationLedger`] instead of running the command again:
//!
//! | Daemon finds                                  | Answer                                   |
//! |-----------------------------------------------|------------------------------------------|
//! | `issued_at` from another daemon session       | `RevisionConflict` / `session_replaced`  |
//! | the id, recorded for the same command         | the original outcome, nothing re-runs    |
//! | the id, recorded for a different command      | `InvalidRequest`                         |
//! | no record, but the ticket predates the ledger | `MutationExpired`, nothing runs          |
//! | no record, and the ticket is within the ledger | the command runs and is recorded        |
//!
//! A committed original is answered with a compact
//! [`RpcResponsePayload::MutationCommitted`] naming the revision it produced,
//! never with the original (possibly large) payload. A rejected original is
//! answered with the same rejection it produced the first time.
//!
//! # Lifetime and limits
//!
//! The ledger is scoped to one daemon session and holds the outcomes of the
//! last [`MUTATION_LEDGER_CAPACITY`] ticketed commands that ran. There is no
//! time-based expiry.
//!
//! The daemon advances the revision for every admitted command that may have
//! changed state, whatever its outcome (a batch that fails part-way keeps its
//! earlier writes). A command recorded without advancing the revision is taken
//! to have changed nothing.
//!
//! Evicting the outcome of a command that advanced the revision moves the
//! ledger's horizon to the revision at which that command ran. A ticket issued
//! after the horizon cannot belong to a forgotten command that changed state,
//! so its absence from the ledger proves that running it now is its only
//! effect. A ticket issued at or before the horizon might belong to such a
//! command, so it is refused with [`RpcErrorCode::MutationExpired`] rather than
//! risk running twice. Evicting a command that changed nothing leaves the
//! horizon alone, so a run of refusals at one revision cannot expire the
//! tickets minted there.
//!
//! In practice a retry expires only when more than
//! [`MUTATION_LEDGER_CAPACITY`] ticketed edits ran between the ticket's
//! `issued_at` and its retry. Commands refused before they ran (a stale
//! precondition, a replaced session, an expired ticket) are not recorded:
//! resending them is refused the same way again.
//!
//! A ticket expires by its `issued_at`, not by when it was sent. A client that
//! mints from a revision it saw long ago, while peers ran more than a ledger's
//! worth of edits, can have a ticket refused as expired on its **first**
//! send. Only the client knows that send was the first. When it was, nothing
//! with that ticket ran, so the client may mint a new ticket from the
//! refusal's revision and send the command once more.
//!
//! # Re-reading after an unknown outcome
//!
//! An expired ticket, a replaced daemon, or a request that was not resent
//! leaves the outcome unknown. The client then re-reads state
//! (`GetInvention`, `InspectInvention`, or a snapshot) and decides whether its
//! change is present. A replacement daemon restores the last persisted
//! session, so an edit that committed before the restart may or may not be
//! present; only a re-read can tell.
//!
//! # What a client resends automatically
//!
//! [`RpcRequestPayload::replay_policy`] is the single classification:
//!
//! - Reads, and saving the document, resend freely.
//! - Authoring commands resend only with their original ticket, and only to
//!   the daemon session that ticket was issued against.
//! - Everything else is never resent automatically. That covers performance
//!   control writes, package installs, and shutdown.
//!
//! Performance control writes are left to the caller because only the caller
//! knows what a value means. An adapter holding an absolute continuous value
//! (a fader position) may send its latest value again after confirming, from
//! a fresh handshake, that the daemon session is unchanged. A discrete trigger
//! (a note, a restart, a toggle standing in for a button press) is never
//! replayed automatically, because firing it twice is audible. Neither is
//! replayed into a replacement daemon: the gesture belonged to a runtime that
//! no longer exists.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use super::{
    ConflictReason, RevisionConflict, RevisionTracker, RpcCommand, RpcError, RpcErrorCode,
    RpcRequestPayload, RpcResponsePayload, RuntimeRevision,
};

/// How many ticketed outcomes a daemon session remembers.
///
/// Sized for recovery after reconnecting, not for history. Each recorded
/// entry is small (an id, a fingerprint, and a revision or a bounded
/// rejection), so the whole ledger stays in the tens of kilobytes.
pub const MUTATION_LEDGER_CAPACITY: usize = 128;

/// The longest accepted [`MutationTicket::id`], in bytes.
pub const MAX_MUTATION_ID_BYTES: usize = 128;

/// The longest rejection message the ledger keeps. Longer messages are cut
/// short when replayed so an entry's size stays bounded.
pub const MAX_RECORDED_MESSAGE_BYTES: usize = 4096;

/// A client's identity for one authoring command, reused unchanged on every
/// retry of that command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct MutationTicket {
    /// Opaque and unique among the client's tickets. Clients sharing a daemon
    /// must not collide, so include something process-unique in it.
    pub id: String,
    /// The latest revision the client had seen when it minted the ticket. Its
    /// session fences the command to that daemon; its counter bounds how far
    /// back the ledger must remember.
    pub issued_at: RuntimeRevision,
}

/// What a client may resend automatically when a request's outcome is
/// unknown. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPolicy {
    /// No lasting effect, or an effect that repeating reproduces exactly.
    Resend,
    /// An authoring command: resend only with its original ticket, and only
    /// to the daemon session the ticket was issued against.
    ResendWithTicket,
    /// Never resend automatically; report the outcome as unknown.
    Never,
}

impl RpcRequestPayload {
    /// The replay rule for this request, shared by every client transport.
    pub fn replay_policy(&self) -> ReplayPolicy {
        match self {
            Self::Hello | Self::GetSnapshot | Self::PollEvents { .. } | Self::GetMeters => {
                ReplayPolicy::Resend
            }
            Self::Subscribe { .. } => ReplayPolicy::Never,
            Self::Command(command) => command.replay_policy(),
        }
    }
}

impl RpcCommand {
    /// The replay rule for this command. Authoring commands need a ticket;
    /// reads and saves resend freely; everything else is never replayed.
    pub fn replay_policy(&self) -> ReplayPolicy {
        if self.advances_revision() {
            return ReplayPolicy::ResendWithTicket;
        }
        match self {
            Self::InspectInvention { .. }
            | Self::GetInvention { .. }
            // Writes the current document again; the file matches current state.
            | Self::SaveInvention { .. }
            | Self::ListPackages
            | Self::ListDevelopments { .. }
            | Self::DescribeDevelopment { .. }
            | Self::ListExamples { .. }
            | Self::DescribeExample { .. }
            | Self::DescribeModuleTypes(_)
            | Self::DescribeModule(_) => ReplayPolicy::Resend,
            // Performance writes (see the module docs), installs, and shutdown,
            // which could stop a replacement daemon.
            _ => ReplayPolicy::Never,
        }
    }
}

/// The ledger's decision about a ticketed command, before it runs.
#[derive(Debug)]
pub enum Admission {
    /// Not seen before: run the command, then pass this to
    /// [`MutationLedger::record`] with the response.
    Execute(PendingMutation),
    /// Already ran: answer with this payload and run nothing.
    Replay(Box<RpcResponsePayload>),
}

/// A ticketed command the ledger admitted and is waiting to record.
#[derive(Debug)]
pub struct PendingMutation {
    id: String,
    fingerprint: u64,
    executed_at: u64,
}

#[derive(Debug)]
enum RecordedOutcome {
    Committed(RuntimeRevision),
    Rejected(Box<RpcResponsePayload>),
}

#[derive(Debug)]
struct LedgerEntry {
    id: String,
    fingerprint: u64,
    executed_at: u64,
    /// Whether the revision moved while the command ran. Only such commands
    /// may have changed state, so only they move the horizon when evicted.
    advanced: bool,
    outcome: RecordedOutcome,
}

/// The daemon's bounded record of ticketed outcomes for one session.
///
/// Owned by the daemon and used only from its request-handling path, under
/// the same serialization as the commands it records. The audio thread never
/// sees it.
#[derive(Debug)]
pub struct MutationLedger {
    entries: VecDeque<LedgerEntry>,
    capacity: usize,
    /// The latest revision at which an evicted command that advanced the
    /// revision ran. `None` until the first such eviction.
    horizon: Option<u64>,
}

impl Default for MutationLedger {
    fn default() -> Self {
        Self::with_capacity(MUTATION_LEDGER_CAPACITY)
    }
}

impl MutationLedger {
    /// A ledger holding at most `capacity` outcomes (at least one).
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            horizon: None,
        }
    }

    /// Decides whether a ticketed command runs, is answered from the record,
    /// or is refused. Call it before the revision precondition, so a retry of
    /// a command that already committed is recognized rather than reported as
    /// stale.
    pub fn admit(
        &self,
        ticket: &MutationTicket,
        command: &RpcCommand,
        revisions: &RevisionTracker,
    ) -> Result<Admission, RpcError> {
        if ticket.id.is_empty() || ticket.id.len() > MAX_MUTATION_ID_BYTES {
            return Err(RpcError::new(
                RpcErrorCode::InvalidRequest,
                format!("mutation id must be 1 to {MAX_MUTATION_ID_BYTES} bytes"),
            ));
        }
        if command.replay_policy() != ReplayPolicy::ResendWithTicket {
            return Err(RpcError::new(
                RpcErrorCode::InvalidRequest,
                "mutation tickets apply to authoring commands only",
            ));
        }
        let current = revisions.current();
        if ticket.issued_at.session_id != current.session_id {
            return Err(RpcError::revision_conflict(RevisionConflict {
                expected: ticket.issued_at.clone(),
                current,
                reason: ConflictReason::SessionReplaced,
            }));
        }
        if ticket.issued_at.revision > current.revision {
            return Err(RpcError::new(
                RpcErrorCode::InvalidRequest,
                format!(
                    "mutation ticket was issued at revision {}, ahead of the daemon's {}",
                    ticket.issued_at.revision, current.revision
                ),
            ));
        }

        let fingerprint = fingerprint(command);
        if let Some(entry) = self.entries.iter().find(|entry| entry.id == ticket.id) {
            if entry.fingerprint != fingerprint {
                return Err(RpcError::new(
                    RpcErrorCode::InvalidRequest,
                    format!(
                        "mutation id '{}' was already used for a different command",
                        ticket.id
                    ),
                ));
            }
            return Ok(Admission::Replay(Box::new(match &entry.outcome {
                RecordedOutcome::Committed(revision) => RpcResponsePayload::MutationCommitted {
                    mutation_id: entry.id.clone(),
                    committed_at: revision.clone(),
                },
                RecordedOutcome::Rejected(payload) => (**payload).clone(),
            })));
        }
        if self
            .horizon
            .is_some_and(|horizon| ticket.issued_at.revision <= horizon)
        {
            return Err(RpcError::new(
                RpcErrorCode::MutationExpired,
                format!(
                    "the outcome of mutation '{}' is no longer recorded; it may or may not \
                     have applied. Re-read the invention before editing again",
                    ticket.id
                ),
            ));
        }
        Ok(Admission::Execute(PendingMutation {
            id: ticket.id.clone(),
            fingerprint,
            executed_at: current.revision,
        }))
    }

    /// Records how an admitted command ended. `after` is the daemon's revision
    /// once the command finished.
    ///
    /// Advance the revision before recording for any command that may have
    /// changed state, including one that failed part-way. A command whose
    /// `after` equals the revision it ran at is taken to have changed nothing
    /// (see the [module docs](self)).
    pub fn record(
        &mut self,
        pending: PendingMutation,
        response: &RpcResponsePayload,
        after: RuntimeRevision,
    ) {
        let advanced = after.revision > pending.executed_at;
        let outcome = match response {
            RpcResponsePayload::Error(error) => {
                RecordedOutcome::Rejected(Box::new(RpcResponsePayload::Error(bounded_error(error))))
            }
            RpcResponsePayload::ContentError { error } => {
                let mut error = error.clone();
                truncate(&mut error.message);
                RecordedOutcome::Rejected(Box::new(RpcResponsePayload::ContentError { error }))
            }
            _ => RecordedOutcome::Committed(after),
        };
        if self.entries.len() == self.capacity {
            if let Some(evicted) = self.entries.pop_front() {
                if evicted.advanced {
                    self.horizon = Some(
                        self.horizon
                            .map_or(evicted.executed_at, |h| h.max(evicted.executed_at)),
                    );
                }
            }
        }
        self.entries.push_back(LedgerEntry {
            id: pending.id,
            fingerprint: pending.fingerprint,
            executed_at: pending.executed_at,
            advanced,
            outcome,
        });
    }

    /// How many outcomes are currently recorded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no outcomes are recorded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Identifies a command's content so a reused id with different content is
/// refused. Stable within one daemon process, which is the ledger's lifetime.
fn fingerprint(command: &RpcCommand) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    match serde_json::to_vec(command) {
        Ok(bytes) => bytes.hash(&mut hasher),
        // Every command serializes; fall back to its debug form regardless.
        Err(_) => format!("{command:?}").hash(&mut hasher),
    }
    hasher.finish()
}

fn bounded_error(error: &RpcError) -> RpcError {
    let mut error = error.clone();
    truncate(&mut error.message);
    error
}

fn truncate(message: &mut String) {
    truncate_on_char_boundary(message, MAX_RECORDED_MESSAGE_BYTES);
}

/// Cuts `message` to at most `max_bytes`, on a UTF-8 character boundary.
pub(crate) fn truncate_on_char_boundary(message: &mut String, max_bytes: usize) {
    if message.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
}
