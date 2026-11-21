//! Which module plays each frame before another reads it, so a cable
//! between them can ramp with its source playing (see [`super`]).

use super::super::ProcessGroup;
use super::Onsets;

/// Whether module `a` plays each frame before module `b` reads it, by
/// `groups` (the process groups, in order) and `group_of` (each module's):
/// `a` is in an earlier group, or ahead of `b` in their feedback group
/// (see [`ahead_in`]).
pub(super) fn runs_ahead(groups: &[ProcessGroup], group_of: &[usize], a: usize, b: usize) -> bool {
    match (group_of.get(a), group_of.get(b)) {
        (Some(ga), Some(gb)) if ga != gb => ga < gb,
        (Some(g), Some(_)) => a != b && groups.get(*g).is_some_and(|group| ahead_in(group, a, b)),
        _ => false,
    }
}

/// Whether member `a` of feedback group `group` plays each frame before
/// member `b` reads it. Sample by sample, the members play in order. In
/// chunks, the cuts emit, the rest play in their own order, then the cuts
/// read their inputs and absorb, unless a cut reads them as it emits too.
pub(super) fn ahead_in(group: &ProcessGroup, a: usize, b: usize) -> bool {
    let first = |order: &[usize]| order.iter().copied().find(|&m| m == a || m == b);
    let Some(chunks) = &group.chunks else {
        return first(&group.members) == Some(a);
    };
    let cut = |m: usize| chunks.cuts.iter().position(|&c| c == m);
    match (cut(a), cut(b)) {
        (_, Some(k)) => !chunks.emit_reads.get(k).copied().unwrap_or(true),
        (Some(_), None) => true,
        (None, None) => first(&chunks.rest) == Some(a),
    }
}

/// Whether a route from `a` into `b` reads the frame `a` plays alongside,
/// as the cable's ramp does, rather than the one before (a back edge of a
/// group played sample by sample).
pub(super) fn reads_alongside(
    groups: &[ProcessGroup],
    group_of: &[usize],
    a: usize,
    b: usize,
) -> bool {
    let chunked = group_of
        .get(b)
        .and_then(|&g| groups.get(g))
        .is_some_and(|group| group.chunks.is_some());
    let together = group_of.get(a) == group_of.get(b);
    runs_ahead(groups, group_of, a, b) || (together && chunked)
}

impl Onsets {
    /// Ramps the cable from `source`, just unpatched from input `port` of
    /// module `mi`, out from `level` of it at sample `now`, by the process
    /// groups (see [`runs_ahead`]), or reports that it cannot: nothing has played
    /// yet, the source no longer runs ahead of `mi`, or there is no room.
    pub(in crate::invention::graph) fn ramp_out(
        &mut self,
        (groups, group_of): (&[ProcessGroup], &[usize]),
        (mi, port): (usize, usize),
        source: (usize, usize),
        level: f32,
        now: u64,
    ) -> bool {
        now > 0
            && runs_ahead(groups, group_of, source.0, mi)
            && self.fall(mi, port, source, level, now)
    }
}
