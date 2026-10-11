//! The session log of applied control requests, for reproducible renders.
//!
//! A live session records every control request the audio thread applied,
//! with the sample it applied at, so an offline render of the same
//! invention can apply each one at that same sample
//! ([`RenderEngine::replay`](super::RenderEngine::replay)) and produce what
//! played, sample for sample.
//!
//! # Where it is recorded
//!
//! On a control thread, from what the audio thread already reports: every
//! request a front door submits is remembered (module, control and value)
//! until its outcome arrives through the outcome channel, and an
//! [`Applied`](crate::control_request::Outcome::Applied) or
//! [`AppliedLate`](crate::control_request::Outcome::AppliedLate) outcome
//! carries the sample it applied at. Outcomes arrive in the order the audio
//! thread applied them, so the log is in apply order too. The audio side
//! does no extra work: it allocates, frees and locks nothing for the log.
//!
//! Perform and author writes are both recorded: both shaped what was heard.
//! Superseded and refused requests are not: they never applied.
//!
//! # Bounded
//!
//! The log holds at most [`REQUEST_LOG_CAPACITY`] requests. When it is full
//! it stops recording and sets [`RequestLog::full`]: what it holds is then
//! still exact up to its last entry, where dropping the oldest instead would
//! leave every later state wrong.
//!
//! # When a replay is exact
//!
//! [`RequestLog::is_exact`] says whether the log accounts for every control
//! request applied in the session. It is not when the log filled, when an
//! applied request could not be recorded (outcomes the audio thread had to
//! drop, or a write evicted before its outcome came), when the graph was
//! edited structurally during the session (a replay runs against the
//! invention as loaded), or when a repeating beat grid applied (it reports
//! its first application only). Writes that never pass through a request
//! (input writes, legacy controls) and wall-clock scripts, agents and code
//! modules (which act again in the render) are outside the log altogether.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::control_request::RtValue;

/// The most applied requests a session log records before it stops.
pub const REQUEST_LOG_CAPACITY: usize = 16_384;

/// A value a request applied, exactly as the module received it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum LoggedValue {
    Number(f32),
    Integer(i32),
    /// A choice's position, or another unsigned control value.
    Index(u32),
    Bool(bool),
}

impl From<RtValue> for LoggedValue {
    fn from(value: RtValue) -> Self {
        match value {
            RtValue::F32(number) => Self::Number(number),
            RtValue::I32(number) => Self::Integer(number),
            RtValue::U32(number) => Self::Index(number),
            RtValue::Bool(flag) => Self::Bool(flag),
        }
    }
}

impl From<LoggedValue> for RtValue {
    fn from(value: LoggedValue) -> Self {
        match value {
            LoggedValue::Number(number) => Self::F32(number),
            LoggedValue::Integer(number) => Self::I32(number),
            LoggedValue::Index(number) => Self::U32(number),
            LoggedValue::Bool(flag) => Self::Bool(flag),
        }
    }
}

/// The musical time a request was submitted for: which beat of which clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "beat", rename_all = "snake_case")]
pub enum LoggedBeat {
    /// `beats` after the clock's position when the audio thread took it.
    After { clock: String, beats: f32 },
    /// The next beat congruent to `offset` modulo `every`, again at every
    /// such beat when it repeats.
    Grid {
        clock: String,
        every: f32,
        offset: f32,
        repeat: bool,
    },
}

/// One applied request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct LoggedRequest {
    /// The engine sample it applied at, counted from the session's start.
    pub sample: u64,
    pub module_id: String,
    pub key: String,
    pub value: LoggedValue,
    /// The sample it was due at, when it applied late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<u64>,
    /// Its musical time, when it was timed in beats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<LoggedBeat>,
}

/// A session's applied requests, oldest first, as exported.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RequestLog {
    pub requests: Vec<LoggedRequest>,
    /// The log filled and stopped recording after its last entry.
    pub full: bool,
    /// Applied requests the log may have missed: outcomes the audio thread
    /// dropped for want of room (counted whether or not they were
    /// applications), and applications of writes it no longer remembered.
    pub lost: u64,
    /// Structural edits published during the session.
    pub edits: u64,
}

impl RequestLog {
    /// Whether a replay against the invention as loaded applies every
    /// request the session applied (see the module docs).
    pub fn is_exact(&self) -> bool {
        let repeats = self
            .requests
            .iter()
            .any(|request| matches!(request.beat, Some(LoggedBeat::Grid { repeat: true, .. })));
        !self.full && self.lost == 0 && self.edits == 0 && !repeats
    }
}

/// The recorder behind a live graph's log. Control threads only.
#[derive(Default)]
pub(crate) struct Recorder {
    requests: VecDeque<LoggedRequest>,
    full: bool,
    lost: u64,
    edits: u64,
}

impl Recorder {
    /// Records an applied request, unless the log is full.
    pub(crate) fn applied(&mut self, request: LoggedRequest) {
        if self.requests.len() == REQUEST_LOG_CAPACITY {
            self.full = true;
            return;
        }
        self.requests.push_back(request);
    }

    /// Counts applied requests that could not be recorded.
    pub(crate) fn lost(&mut self, count: u64) {
        self.lost += count;
    }

    /// Counts a structural edit published.
    pub(crate) fn edited(&mut self) {
        self.edits += 1;
    }

    /// The log as recorded so far.
    pub(crate) fn export(&self) -> RequestLog {
        RequestLog {
            requests: self.requests.iter().cloned().collect(),
            full: self.full,
            lost: self.lost,
            edits: self.edits,
        }
    }
}
