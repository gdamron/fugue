//! Timed structural edits on the wire: when a swap, an edit batch or a
//! reload lands, and what became of it.
//!
//! A command carrying a `when` is queued rather than applied: the reply is
//! a [`RequestStatus`] naming the request, and every later change of state
//! is announced as a `RequestChanged` event and readable with
//! [`RpcCommand::GetRequest`](super::RpcCommand::GetRequest). The shape is
//! the timed-edit subset of the control request contract (FUG-317): a
//! request id, where it lands, and one state.

use serde::{Deserialize, Serialize};

use crate::LoggedBeat;

/// When a structural edit lands: at once, the default, or at `when`, with
/// the reply then the queued request's [`RequestStatus`]. Flattened into
/// the commands that take it, so a command without it reads and writes
/// exactly as before.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct EditTiming {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "refuse_until_runnable"
    )]
    #[cfg_attr(feature = "rpc-schema", schemars(with = "Option<EditWhen>"))]
    pub when: Option<EditWhen>,
    /// With `when`: expired once this many milliseconds pass without it
    /// landing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// Refuses a `when` this build cannot run. The contract lands before the
/// runtime that lands a timed edit, and a host given a `when` it cannot
/// honour would apply the edit at once: so the wire refuses it, clearly,
/// until the runtime path is in the same build.
fn refuse_until_runnable<'de, D>(deserializer: D) -> Result<Option<EditWhen>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<EditWhen>::deserialize(deserializer)? {
        None => Ok(None),
        Some(_) => Err(serde::de::Error::custom(
            "timed edits (`when`) are not supported by this build yet: send the edit without `when` to apply it at once",
        )),
    }
}

impl EditTiming {
    /// Whether the edit is timed rather than applied at once.
    pub fn is_timed(&self) -> bool {
        self.when.is_some()
    }
}

impl super::RpcCommand {
    /// The timing of a swap, an edit batch or a reload; `None` for any
    /// other command.
    pub fn edit_timing(&self) -> Option<&EditTiming> {
        match self {
            Self::SwapModule { timing, .. }
            | Self::ApplyEdits { timing, .. }
            | Self::ReloadInvention { timing, .. } => Some(timing),
            _ => None,
        }
    }

    /// Whether this is a timed structural edit: queued rather than applied,
    /// replying with the request's [`RequestStatus`].
    pub fn is_timed(&self) -> bool {
        self.edit_timing().is_some_and(EditTiming::is_timed)
    }
}

/// When a timed structural edit lands. Every beat time counts on a clock:
/// the one named, otherwise the edited module's clock for a swap, or the
/// invention's first clock for an edit batch or a reload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditWhen {
    /// On this sample of the transport.
    AtSample { sample: u64 },
    /// This many samples after the audio thread takes the edit.
    AfterSamples { samples: u64 },
    /// This many beats (not negative) after the clock's position.
    AfterBeats {
        beats: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock: Option<String>,
    },
    /// The next beat past the clock's position that is `offset` beats past
    /// a multiple of `every` (positive), counted from the clock's beat 0.
    NextBeat {
        every: f32,
        #[serde(default)]
        offset: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock: Option<String>,
    },
    /// The next bar's downbeat: the next multiple of `beats_per_bar`
    /// (default 4), counted from the clock's beat 0. The engine keeps no
    /// meter, so a bar is only this many beats.
    NextBar {
        #[serde(default = "default_beats_per_bar")]
        beats_per_bar: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock: Option<String>,
    },
}

fn default_beats_per_bar() -> f32 {
    4.0
}

/// Where a timed request stands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    /// Waiting for its time. It may still be cancelled, refused or expire.
    Queued,
    /// Playing since `landing.sample`; the document and snapshot follow
    /// within moments, as it is adopted.
    Landed,
    /// Playing, and the document, the snapshot and the revision describe
    /// it. Final.
    Adopted,
    /// Never applied; `reason` says why. Final.
    Refused,
    /// Its ttl passed before its time came. Final.
    Expired,
    /// Another change cancelled it before it landed; `reason` says which.
    /// Final.
    Cancelled,
}

impl RequestState {
    /// Whether it can change no more.
    pub fn is_final(self) -> bool {
        !matches!(self, Self::Queued | Self::Landed)
    }
}

/// Where a timed request lands, as far as is known.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RequestLanding {
    /// The transport sample it lands on: known up front when timed to a
    /// sample, otherwise once it has landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<u64>,
    /// The sample it was due on, when it landed later than that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<u64>,
    /// Its musical time, with the clock it counts on, when timed in beats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<LoggedBeat>,
}

/// A timed request's status: the reply to a timed command and to
/// [`RpcCommand::GetRequest`](super::RpcCommand::GetRequest), and the body
/// of each `RequestChanged` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RequestStatus {
    /// Names the request in `GetRequest` and in `RequestChanged` events.
    /// Unique only within one running invention: a host that keeps
    /// requests across runs must qualify it with the run.
    pub id: String,
    pub state: RequestState,
    #[serde(default)]
    pub landing: RequestLanding,
    /// Why it was refused, expired or cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Modules it added or rebuilt, once adopted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built: Vec<String>,
    /// Modules it removed, once adopted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}
