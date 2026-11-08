//! The types a write to a declared surface takes and returns.

use crate::control_request::{ControlIndex, RequestId, RtValue, Source};
use crate::invention::publish::{Authoring, WriteTime};
use crate::payload::Payload;
use crate::ControlValue;

/// What a write saw as it was queued for the live graph, under the
/// publisher's lock: the timed edit the write is safe from (see
/// `Publisher::keep_authored`), and the request it was submitted as.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Queued {
    pub(crate) safe: Option<RequestId>,
    /// The request the write was submitted as, whose outcome says when it
    /// applied; `None` when it applied as it was made (building, offline,
    /// an undeclared control) or travels in a publication, which reports none.
    pub(crate) id: Option<RequestId>,
    /// An offline write that lost to a higher-ranked write at the same
    /// sample, so nothing was applied (see `control_request::hold`).
    pub(crate) lost: bool,
    /// The publication a write travels in instead of a request (see
    /// `publish::retarget`): it applies once the audio thread installs
    /// that generation.
    pub(crate) published: Option<u64>,
}

/// How a front door submits a write: whether it is authored (and what the
/// retained document records once it applies), who wrote it and its rank
/// within that tier (see `control_request::hold`), how many samples it
/// may wait before it is refused `Expired`, and whether it is announced
/// once it applies (see `PendingWrite::announced`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WriteOptions<'a> {
    pub(crate) authored: Option<&'a Authoring>,
    pub(crate) source: Source,
    pub(crate) priority: i16,
    pub(crate) ttl: Option<u64>,
    pub(crate) announced: bool,
    /// A reply awaits its outcome (see `PendingWrite::awaited`).
    pub(crate) awaited: bool,
    /// When it applies, if not as soon as the audio thread takes it: a
    /// timed write, which only a live graph's declared, value or payload
    /// control takes (see `publish::timed_write`).
    pub(crate) time: Option<WriteTime<'a>>,
    /// The sample an offline payload write was due at, which it holds
    /// from when earlier than now, as a request applied late does: a
    /// replayed one (see `render::replay`).
    pub(crate) due: Option<u64>,
}

impl<'a> WriteOptions<'a> {
    /// A performed write from `source` at priority 0, with no ttl.
    pub(crate) const fn performed(source: Source) -> Self {
        Self {
            authored: None,
            source,
            priority: 0,
            ttl: None,
            announced: false,
            awaited: false,
            time: None,
            due: None,
        }
    }

    /// The same write, authored: recorded as `authoring` says once it
    /// applies.
    pub(crate) const fn authored_as<'b>(self, authoring: &'b Authoring) -> WriteOptions<'b>
    where
        'a: 'b,
    {
        WriteOptions {
            authored: Some(authoring),
            source: self.source,
            priority: self.priority,
            ttl: self.ttl,
            announced: self.announced,
            awaited: self.awaited,
            time: self.time,
            due: self.due,
        }
    }

    /// The same write, announced once it applies.
    pub(crate) const fn announcing(self) -> Self {
        Self {
            announced: true,
            ..self
        }
    }

    /// The same write, its outcome awaited by a reply.
    pub(crate) const fn awaiting(self) -> Self {
        Self {
            awaited: true,
            ..self
        }
    }

    /// The same write, performed.
    pub(crate) const fn performing(self) -> Self {
        Self {
            authored: None,
            ..self
        }
    }
}

impl Default for WriteOptions<'_> {
    /// A user's performed write: what a control write without options is.
    fn default() -> Self {
        Self::performed(Source::User)
    }
}

/// A write resolved for a request the live graph times itself.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Resolved {
    pub(crate) index: ControlIndex,
    pub(crate) raw: RtValue,
    pub(crate) event: bool,
    /// Whether the control is reserved (see `ControlDecl::reserved`).
    pub(crate) reserved: bool,
    /// Its key and value as reads list it pending.
    pub(crate) shown: Option<(String, ControlValue)>,
}

/// A write on its way to a module: a value, or a payload and what reads
/// report for it, checked as [`Checked`] says when built against the
/// directory.
pub(super) enum Write<'a> {
    Value(RtValue),
    Payload(Payload, ControlValue, Option<Checked<'a>>),
}

/// A payload write built against the live graph's control-surface
/// directory (see [`super::DeclaredSurface::write_payload_checked`]).
pub(crate) struct Checked<'a> {
    /// The publisher generation the directory was read at
    /// ([`super::DeclaredSurface::generation`]); a live write the publisher would
    /// stamp with any other is refused as [`STALE`], since a graph change
    /// may have replaced what the payload resolved; so is an offline write
    /// after an edit installed in place since.
    pub(crate) generation: Option<u64>,
    /// The modules the payload writes: a timed edit waiting that rebuilds
    /// or removes one, or that re-resolved the payload this write
    /// replaces, is cancelled (see `Publisher::timed_races`). On a live
    /// graph, a topology compiled with the module writing others is
    /// recompiled with these and published just ahead of the write (see
    /// `publish::retarget`).
    pub(crate) targets: Vec<String>,
    /// The most ramps the payload's schedule can run at once: on a live
    /// graph, a drain with less room for its tables' ramps takes more with
    /// a topology published just ahead of the write, as for `targets`.
    pub(crate) ramps: usize,
    /// Records the write where it commits: under the publisher on a live
    /// graph, so a graph change sees every write submitted before it
    /// recorded, and none after.
    pub(crate) record: Box<dyn FnOnce() + 'a>,
}

/// A live payload write built against a directory a graph change has
/// replaced since: build it again.
pub(crate) const STALE: &str = "The graph changed while this write was prepared; try again";
