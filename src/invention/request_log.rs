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
//! Superseded, overridden and refused requests are not: they never
//! applied. (A request overridden at its due sample lost to a write that
//! applied there, which is logged, or came late and was weighed at a block
//! start: it splits no block the log does not.)
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
//! [`RequestLog::is_exact`] says whether a replay can account for every
//! change made to the session. It is not when:
//!
//! - the log filled, or an applied request could not be recorded (outcomes
//!   the audio thread had to drop, or a write evicted before its outcome
//!   came): [`RequestLog::full`], [`RequestLog::lost`];
//! - a write reached the graph without a request (an input write, or a
//!   control a module has not yet moved onto declared controls), or applied
//!   a payload (a step sequencer's `pattern`), which a [`LoggedValue`]
//!   cannot carry, or applied a request at the automation tier, which a
//!   replay (holding every logged write above automation) cannot
//!   reproduce: [`RequestLog::unlogged`];
//! - the graph was edited, reloaded or given an edit batch at once during
//!   the session (a replay runs against the invention as loaded):
//!   [`RequestLog::edits`]. A timed one is logged with its outcome instead
//!   (see [`LoggedEdit`]), and a replay lands it on the sample it landed
//!   on, or leaves it out when it was refused;
//! - the invention runs code or agent modules, whose wall-clock writes are
//!   logged and would act again in the render: [`RequestLog::actors`];
//! - a repeating beat grid applied (it reports its first application only).
//!
//! A write's hold against a control scheduler is no exception: a replay
//! holds each control as the session did (see the replay's docs).
//!
//! It also assumes that what live playback split each block at is what the
//! replay splits at: the logged samples, on a grid of
//! [`RequestLog::block_size`] frames from sample 0, which holds while every
//! device callback is a whole number of blocks. A module whose output
//! depends on how a block is split (a control scheduler keeps the last
//! write of each piece it processes) can tell a split the log does not
//! record: the due sample of a request refused when it came to apply.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::control_request::RtValue;

mod timed;

pub use timed::{LoggedConnection, LoggedEdit, LoggedEditOutcome, LoggedModule, LoggedWhen};

/// The most applied requests a session log records before it stops.
pub const REQUEST_LOG_CAPACITY: usize = 16_384;

/// The most timed edits a session log records before it stops.
pub const TIMED_EDIT_LOG_CAPACITY: usize = 1_024;

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
#[serde(tag = "kind", rename_all = "snake_case")]
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
    /// The session's sample rate: a replay renders at this rate.
    #[serde(default)]
    pub sample_rate: u32,
    /// The session's block size in frames: a replay renders with this one.
    #[serde(default)]
    pub block_size: usize,
    #[serde(default)]
    pub requests: Vec<LoggedRequest>,
    /// The log filled and stopped recording after its last entry.
    #[serde(default)]
    pub full: bool,
    /// Applied requests the log may have missed: outcomes the audio thread
    /// dropped for want of room (counted whether or not they were
    /// applications), and applications of writes it no longer remembered.
    #[serde(default)]
    pub lost: u64,
    /// Writes that reached the graph without a logged request: input
    /// writes, writes to controls not declared, applied payload writes
    /// (a step sequencer's `pattern`), which a [`LoggedValue`] cannot carry,
    /// and applied automation-tier requests, which a replay cannot hold as
    /// they held.
    #[serde(default)]
    pub unlogged: u64,
    /// Commits attempted on the graph at once during the session
    /// (structural edits, reloads and edit batches), whether or not they
    /// landed, and timed ones the log could not record (see
    /// [`LoggedEdit::queued`]). A log from before timed edits were recorded
    /// counts those here too, so it still replays as inexact.
    #[serde(default)]
    pub edits: u64,
    /// Timed structural edits, in the order they settled, landed or not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timed_edits: Vec<LoggedEdit>,
    /// Code and agent modules in the invention, which write on wall-clock
    /// time and would write again in a replay.
    #[serde(default)]
    pub actors: u64,
}

impl RequestLog {
    /// Whether a replay against the invention as loaded can reproduce the
    /// session (see the module docs).
    pub fn is_exact(&self) -> bool {
        let repeats = self
            .requests
            .iter()
            .any(|request| matches!(request.beat, Some(LoggedBeat::Grid { repeat: true, .. })));
        let missed = self.lost + self.unlogged + self.edits + self.actors;
        !self.full && missed == 0 && !repeats
    }
}

/// The recorder behind a live graph's log. Control threads only.
#[derive(Default)]
pub(crate) struct Recorder {
    requests: VecDeque<LoggedRequest>,
    timed_edits: Vec<LoggedEdit>,
    full: bool,
    lost: u64,
    unlogged: u64,
    edits: u64,
}

impl Recorder {
    /// Records an applied request, unless the log is full.
    pub(crate) fn applied(&mut self, request: LoggedRequest) {
        if self.full || self.requests.len() == REQUEST_LOG_CAPACITY {
            self.full = true;
            return;
        }
        self.requests.push_back(request);
    }

    /// Records a timed edit as it settled, unless the log is full.
    pub(crate) fn timed_edit(&mut self, edit: LoggedEdit) {
        if self.full || self.timed_edits.len() == TIMED_EDIT_LOG_CAPACITY {
            self.full = true;
            return;
        }
        self.timed_edits.push(edit);
    }

    /// Counts applied requests that could not be recorded.
    pub(crate) fn lost(&mut self, count: u64) {
        self.lost += count;
    }

    /// Counts a write that reached the graph without a request.
    pub(crate) fn unlogged(&mut self) {
        self.unlogged += 1;
    }

    /// Counts a commit attempted on the graph.
    pub(crate) fn edited(&mut self) {
        self.edits += 1;
    }

    /// The log as recorded so far; the session adds its rate, block size
    /// and actors.
    pub(crate) fn export(&self) -> RequestLog {
        RequestLog {
            requests: self.requests.iter().cloned().collect(),
            full: self.full,
            lost: self.lost,
            unlogged: self.unlogged,
            edits: self.edits,
            timed_edits: self.timed_edits.clone(),
            ..RequestLog::default()
        }
    }
}
