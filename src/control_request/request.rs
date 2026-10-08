//! The typed control request.

use std::time::Instant;

use crate::payload::Payload;

/// A real-time control value: `Copy` and small, so carrying, coalescing and
/// applying one never allocates or frees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RtValue {
    F32(f32),
    U32(u32),
    I32(i32),
    Bool(bool),
}

/// A control's position in its module's declared
/// [`ControlTable`](super::ControlTable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ControlIndex(pub(crate) u16);

/// The control a request writes.
///
/// Resolved on a control thread against the publisher's mirror as of
/// `generation`, exactly like an
/// [`InputWrite`](crate::invention::graph::InputWrite): `module_idx` is the
/// module's position in that generation's module order, and the audio side
/// maps it across later generations the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ControlTarget {
    pub(crate) generation: u64,
    pub(crate) module_idx: usize,
    pub(crate) control: ControlIndex,
}

/// When a request applies, on the engine's sample transport (see
/// [`Transport`](super::Transport)).
///
/// This is the extension point for musical time: beats on a timeline and
/// conditions arrive with the Musical Time project (B2–B4) as new variants.
///
/// A request whose sample has already passed when the audio thread takes
/// it still applies, at the first segment start after it, and reports
/// [`Outcome::AppliedLate`](super::Outcome::AppliedLate), unless its `ttl`
/// has run out.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum When {
    /// At the start of the next block the audio thread drains.
    Now,
    /// At this value of the engine's sample counter
    /// (`SignalGraph::current_sample`).
    AtSample(u64),
    /// This many samples after the count published when the request is
    /// submitted ([`Transport::rendered`](super::Transport::rendered)).
    /// [`RequestSender::submit`](super::RequestSender::submit) resolves it
    /// to [`When::AtSample`].
    AfterSamples(u64),
    /// The sample heard at this wall-clock time, within the bound
    /// [`Transport::sample_at`](super::Transport::sample_at) documents.
    /// [`RequestSender::submit`](super::RequestSender::submit) resolves it
    /// to [`When::AtSample`]; without a wall clock (offline render) it stays
    /// as it is and is refused
    /// ([`Refusal::NoClock`](super::Refusal::NoClock)).
    AtTime(Instant),
}

/// Whether a write changes the composition or performs on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    /// The value becomes the invention's new starting state and is recorded
    /// when it is applied.
    Author,
    /// A live gesture: applied and announced, never recorded.
    Perform,
}

/// Who submitted a request.
///
/// Priority tiers order sources user = agent > script > automation; the
/// override rules that apply them are A4's (FUG-316), not this module's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    User,
    Agent,
    Script,
    Automation,
}

/// Identifies a submitted request in outcomes. Unique per
/// [`RequestSender`](super::RequestSender) channel; `0` means not yet
/// submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RequestId(pub(crate) u64);

/// What a request writes.
///
/// Not `Copy` or `Clone`: a payload is moved, never duplicated. On the
/// audio side a payload leaves only by being kept by the module it applies
/// to or by being retired (see `crate::payload`), never by being dropped.
#[derive(Debug)]
pub(crate) enum RequestValue {
    Value(RtValue),
    /// A heavy value prepared on a control thread (FUG-309).
    Payload(Payload),
}

impl RequestValue {
    pub(crate) fn is_payload(&self) -> bool {
        matches!(self, Self::Payload(_))
    }
}

/// One control write on its way to the audio thread.
///
/// Not `Copy` or `Clone`, so its payload has exactly one owner.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) target: ControlTarget,
    pub(crate) value: RequestValue,
    pub(crate) when: When,
    pub(crate) intent: Intent,
    pub(crate) source: Source,
    /// Orders requests within a source's tier (A4); higher wins.
    pub(crate) priority: i16,
    /// How many samples after the count published at submission the
    /// request may still apply; past that it is refused
    /// ([`Refusal::Expired`](super::Refusal::Expired)).
    /// [`RequestSender::submit`](super::RequestSender::submit) resolves it
    /// into [`Self::expires`] and clears it.
    pub(crate) ttl: Option<u64>,
    /// Whether it fires an event (its control is declared an event, see
    /// [`ControlDecl::event`](super::ControlDecl)): never coalesced with
    /// another request for the same control and sample.
    pub(crate) event: bool,
    /// The last sample the request may apply at. Set from [`Self::ttl`] at
    /// submission; the audio thread reads only this.
    pub(crate) expires: Option<u64>,
    /// Assigned by [`RequestSender::submit`](super::RequestSender::submit).
    pub(crate) id: RequestId,
}

impl Request {
    /// A user's immediate, performed write at priority 0 with no ttl.
    pub(crate) fn new(target: ControlTarget, value: RequestValue) -> Self {
        Self {
            target,
            value,
            when: When::Now,
            intent: Intent::Perform,
            source: Source::User,
            priority: 0,
            ttl: None,
            event: false,
            expires: None,
            id: RequestId(0),
        }
    }
}
