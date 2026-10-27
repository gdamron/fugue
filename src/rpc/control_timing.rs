//! Timed control writes on the wire: when a `SetControl` or a write of a
//! `SetControls` batch applies, and what became of it.
//!
//! A write carrying a `when` waits on the audio thread for its time. Its
//! reply names the request and where it lands (see
//! [`AppliedControl::landing`](super::AppliedControl::landing)), the
//! pending listing shows it while it waits, and it is announced once as it
//! settles, as a `ControlRequestSettled` event: applied (with a
//! `ControlChanged` event too), superseded, overridden, refused or expired.
//! An authored write is recorded in the document as it applies, not as it
//! is made.

use serde::{Deserialize, Serialize};

use super::{ControlRequestState, RequestLanding};

/// When a control write applies: as soon as the audio thread takes it, the
/// default, or at `when`. Flattened into the commands that take it, so a
/// write without it reads and writes exactly as before.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ControlTiming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<ControlWhen>,
    /// With `when`: expired, and never applied, once this many
    /// milliseconds pass without it applying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

impl ControlTiming {
    /// Whether the write says anything about its time: a `when` or a ttl.
    pub fn is_set(&self) -> bool {
        self.when.is_some() || self.ttl_ms.is_some()
    }
}

/// When a timed control write applies. Every beat time counts on a clock:
/// the one named, otherwise the written module's clock, or the invention's
/// first clock for a module that has none.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlWhen {
    /// On this sample of the transport.
    AtSample { sample: u64 },
    /// This many samples after the audio thread takes the write.
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
    /// (default 4), counted from the clock's beat 0.
    NextBar {
        #[serde(default = "default_beats_per_bar")]
        beats_per_bar: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clock: Option<String>,
    },
    /// `offset_samples` after the sample another request applies on: a
    /// control write's or a timed edit's, named as its reply named it. One
    /// applied already counts from where it applied; one that never applies
    /// (refused, superseded, expired, cancelled) refuses this write too, as
    /// does a name no request still known has.
    AfterRequest {
        request: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        offset_samples: u64,
    },
}

fn default_beats_per_bar() -> f32 {
    4.0
}

fn is_zero(samples: &u64) -> bool {
    *samples == 0
}

/// What became of a timed control write, announced once as it settles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ControlRequestSettled {
    /// The request its reply named.
    pub request: String,
    pub module_id: String,
    pub key: String,
    /// Never `pending`.
    pub state: ControlRequestState,
    /// Where it applied, for one that did.
    #[serde(default)]
    pub landing: RequestLanding,
    /// Why one refused or expired never applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
