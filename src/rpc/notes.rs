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
    /// What `start`, `end` and `playhead` count: beats on the module's
    /// clock, or seconds when it has none.
    pub time_unit: NoteTimeUnit,
    /// The clock module its beats count on, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<String>,
    /// Notes that started or ended after the cursor, each once as it now
    /// stands, oldest change first. A note seen before carries the same
    /// `id` and replaces it. With `resync_required`, every note the daemon
    /// still holds for the module instead.
    pub notes: Vec<SeqNote>,
    /// Newest sequence number the daemon has assigned to a note change.
    /// Pass it as the next poll's `after`. Like event sequence numbers, a
    /// daemon may start anywhere, so a cursor from an earlier daemon is
    /// never mistaken for a current one.
    pub latest_seq: u64,
    /// The cursor cannot be continued: changes after it were pruned or
    /// missed, or it came from another daemon or an earlier run. Drop the
    /// notes held for this module and take `notes` in their place.
    pub resync_required: bool,
    /// Where the module's clock is now, in `time_unit`; `None` without a
    /// clock or while nothing plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playhead: Option<f64>,
}

/// What a [`NotePage`]'s times count.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum NoteTimeUnit {
    /// Beats on the module's clock, counted on across its resets.
    Beats,
    /// Seconds since the invention started playing.
    Seconds,
}

/// One note as it now stands, tagged with the sequence number of its latest
/// change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SeqNote {
    pub seq: u64,
    /// Names the note within its module: its start and its end share it.
    pub id: String,
    /// The voice playing it: `"0"` for a module's lone `gate`, `"N"` for
    /// its `gate.N`.
    pub voice: String,
    /// MIDI pitch, 0 to 127 (60 is middle C), to the nearest note.
    pub pitch: u8,
    /// How hard it was struck, 0 to 1.
    pub velocity: f32,
    pub start: f64,
    /// `None` while it is held.
    #[serde(default)]
    pub end: Option<f64>,
}
