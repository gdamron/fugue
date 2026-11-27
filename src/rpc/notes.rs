//! The notes a module plays, for a view that follows them: a piano roll
//! over a sequencer, say.
//!
//! A client asks for a module's notes changed since its cursor
//! ([`RpcRequestPayload::PollNotes`]) and gets a [`NotePage`]: each note
//! that started, or ended, since then, as it now stands, and the playhead.
//! The daemon keeps a bounded history of each module's notes, so a client
//! that falls behind is told so and handed the whole history in place of
//! what it holds.
//!
//! [`RpcRequestPayload::PollNotes`]: super::RpcRequestPayload::PollNotes

use serde::{Deserialize, Serialize};

/// Reply to [`RpcRequestPayload::PollNotes`]: one module's notes that
/// changed after the requested cursor, and the cursor to poll from next.
///
/// [`RpcRequestPayload::PollNotes`]: super::RpcRequestPayload::PollNotes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct NotePage {
    pub module_id: String,
    /// The clock module its notes count beats on, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<String>,
    /// Notes that started or ended after the cursor, each once as it now
    /// stands, oldest change first. A note seen before carries the same
    /// `id` and replaces it. With `resync_required`, every note the daemon
    /// still holds for the module instead.
    pub notes: Vec<PlayedNote>,
    /// Newest sequence number the daemon has assigned to a note change.
    /// Pass it as the next poll's `after`. Like event sequence numbers, a
    /// daemon may start anywhere, so a cursor from an earlier daemon is
    /// never mistaken for a current one.
    pub latest_seq: u64,
    /// The cursor cannot be continued: changes after it were pruned or
    /// missed, or it came from another daemon or an earlier run. Drop the
    /// notes held for this module and take `notes` in their place.
    pub resync_required: bool,
    /// Where the module's clock is now, in beats on `clock`; `None` without
    /// a clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playhead: Option<f64>,
}

/// A note as both note tools give it: one played (`get_notes`, a
/// [`PlayedNote`]) and one still to come (`preview_notes`). Beats count on
/// the note's clock across its resets; seconds count from the start of the
/// piece. Times are rounded to the millionth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ListedNote {
    /// The module playing it.
    pub module_id: String,
    /// The voice playing it: `"0"` for a module's lone `gate`, `"N"` for
    /// its `gate.N`.
    pub voice: String,
    /// MIDI pitch, 0 to 127 (60 is middle C), to the nearest note.
    pub pitch: u8,
    /// How hard it was struck, 0 to 1, to four places.
    pub velocity: f64,
    /// The clock its beats count on; `None` without one.
    pub clock: Option<String>,
    /// `None` without a clock.
    pub start_beats: Option<f64>,
    /// `None` without a clock, or while it is held.
    pub end_beats: Option<f64>,
    pub start_seconds: f64,
    /// `None` while it is held.
    pub end_seconds: Option<f64>,
}

/// A note played, as it now stands, tagged with the sequence number of its
/// latest change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PlayedNote {
    pub seq: u64,
    /// Names the note within its module: its start and its end share it.
    pub id: String,
    #[serde(flatten)]
    pub note: ListedNote,
}
