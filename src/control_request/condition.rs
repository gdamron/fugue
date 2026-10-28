//! What a request timed by a condition waits for ([`When::Condition`]).
//!
//! A [`Condition`] is data, not code: up to [`MAX_PARTS`] [`Predicate`]s
//! joined by a [`Join`]. The table that watches them
//! ([`Conditions`](super::Conditions)) evaluates every predicate kind
//! through one interface, so a new predicate is a new variant here and an
//! arm where it is read, and a new combinator (any, not, then) is a new
//! [`Join`]: neither changes the drain.
//!
//! Every predicate is an event: it is met at one sample and stays met. A
//! condition is met at the sample its join says (for [`Join::All`], when
//! the last of its parts is), and its request applies there.
//!
//! [`When::Condition`]: super::When::Condition

use super::pending::Refusal;
use super::request::{ControlIndex, RequestId};

/// The most parts one [`Condition`] joins.
pub(crate) const MAX_PARTS: usize = 4;

/// The level a gate or trigger is high above, as modules read one: a rising
/// edge goes from at most this to more than it.
pub(crate) const GATE_LEVEL: f32 = 0.5;

/// Which way a value crosses a level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    /// From at most the level to above it.
    Rising,
    /// From at least the level to below it.
    Falling,
}

impl Edge {
    /// Whether going from `last` to `now` crosses `level` this way.
    #[inline]
    pub(crate) fn crosses(self, last: f32, now: f32, level: f32) -> bool {
        match self {
            Self::Rising => last <= level && now > level,
            Self::Falling => last >= level && now < level,
        }
    }
}

/// One thing a condition waits for. Modules are named by their position in
/// the order of the request's target generation, and map across
/// generations with the target, as a beat's clock does.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Predicate {
    /// The `count`th time output `port` of `module` crosses `level` the
    /// `edge` way, counted from the sample the audio thread takes the
    /// request: met on the exact sample of that crossing. Counting a
    /// gate's or a clock's beats is [`Self::edges`]; a threshold on an
    /// output is a count of one.
    Signal {
        module: u32,
        port: u16,
        edge: Edge,
        level: f32,
        count: u32,
    },
    /// Control `control` of `module` crossing `level` the `edge` way, read
    /// where controls change: at segment starts.
    Control {
        module: u32,
        control: ControlIndex,
        edge: Edge,
        level: f32,
    },
    /// The request with this id applying: met at the sample it applies.
    Applied(RequestId),
}

impl Predicate {
    /// The `count`th rising edge of a gate or trigger output: a note, a
    /// step, or a clock's beat.
    pub(crate) fn edges(module: u32, port: u16, count: u32) -> Self {
        Self::Signal {
            module,
            port,
            edge: Edge::Rising,
            level: GATE_LEVEL,
            count,
        }
    }

    /// The module whose state it reads, if any.
    pub(crate) fn module(&self) -> Option<u32> {
        match *self {
            Self::Signal { module, .. } | Self::Control { module, .. } => Some(module),
            Self::Applied(_) => None,
        }
    }

    /// The module it reads, mapped as [`Self::module`] names it.
    pub(crate) fn module_mut(&mut self) -> Option<&mut u32> {
        match self {
            Self::Signal { module, .. } | Self::Control { module, .. } => Some(module),
            Self::Applied(_) => None,
        }
    }

    /// Refuses a predicate nothing can meet: a count of zero, a level that
    /// is not finite.
    fn check(&self) -> Result<(), Refusal> {
        match *self {
            Self::Signal { count: 0, .. } => Err(Refusal::Invalid),
            Self::Signal { level, .. } | Self::Control { level, .. } if !level.is_finite() => {
                Err(Refusal::Invalid)
            }
            _ => Ok(()),
        }
    }
}

/// How a condition's parts combine. `All` is v1's; others (any, not, a
/// sequence) are new variants.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Join {
    /// Met once every part is, at the sample the last one was.
    All,
}

impl Join {
    /// The sample a condition is met at, from the sample each part was met
    /// at (`None`: not yet), or `None` while it is not.
    pub(crate) fn met(self, mut parts: impl Iterator<Item = Option<u64>>) -> Option<u64> {
        match self {
            Self::All => parts.try_fold(0, |at, met| met.map(|met| at.max(met))),
        }
    }
}

/// Up to [`MAX_PARTS`] predicates and how they join. `Copy` and fixed in
/// size, so it rides in a [`Request`](super::Request) like any time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Condition {
    join: Join,
    len: u8,
    /// Samples after it is met that its request applies: 0, on the sample
    /// it is met.
    offset: u32,
    /// Whether it re-asserts a write kept over a timed edit (see
    /// [`Self::kept`]).
    kept: bool,
    parts: [Predicate; MAX_PARTS],
}

impl Condition {
    /// `parts` joined by `join`, or `None` for none or more than
    /// [`MAX_PARTS`].
    pub(crate) fn new(join: Join, parts: &[Predicate]) -> Option<Self> {
        let first = *parts.first()?;
        if parts.len() > MAX_PARTS {
            return None;
        }
        let mut all = [first; MAX_PARTS];
        all[..parts.len()].copy_from_slice(parts);
        Some(Self {
            join,
            len: parts.len() as u8,
            offset: 0,
            kept: false,
            parts: all,
        })
    }

    /// The copy of a write kept over timed edit `edit` (see
    /// `Publisher::keep_authored`): met as the edit lands, and only if
    /// `write`, the write it copies, applied; when that settles otherwise
    /// the copy is refused ([`Refusal::ChainBroken`]). A later copy for the
    /// same control over the same edit replaces it (see
    /// [`Self::kept_over`]).
    pub(crate) fn kept(edit: RequestId, write: RequestId) -> Self {
        let parts = [Predicate::Applied(edit), Predicate::Applied(write)];
        Self {
            kept: true,
            ..Self::all(&parts).expect("two parts")
        }
    }

    /// The timed edit a kept copy ([`Self::kept`]) is kept over.
    pub(crate) fn kept_over(&self) -> Option<RequestId> {
        match self.parts[0] {
            Predicate::Applied(edit) if self.kept => Some(edit),
            _ => None,
        }
    }

    /// Met when every one of `parts` is.
    pub(crate) fn all(parts: &[Predicate]) -> Option<Self> {
        Self::new(Join::All, parts)
    }

    pub(crate) fn join(&self) -> Join {
        self.join
    }

    /// The same condition, its request applying `samples` after it is met.
    pub(crate) fn delayed(self, samples: u32) -> Self {
        Self {
            offset: samples,
            ..self
        }
    }

    /// The sample its request is due on, from the sample each part was met
    /// at (`None`: not yet): `offset` after it is met, or `None` while it
    /// is not.
    pub(crate) fn due(&self, parts: impl Iterator<Item = Option<u64>>) -> Option<u64> {
        let met = self.join.met(parts)?;
        Some(met.saturating_add(u64::from(self.offset)))
    }

    pub(crate) fn parts(&self) -> &[Predicate] {
        &self.parts[..usize::from(self.len)]
    }

    pub(crate) fn parts_mut(&mut self) -> &mut [Predicate] {
        &mut self.parts[..usize::from(self.len)]
    }

    /// Refuses a condition with a part nothing can meet.
    pub(crate) fn check(&self) -> Result<(), Refusal> {
        self.parts().iter().try_for_each(Predicate::check)
    }
}

#[cfg(test)]
mod tests;
