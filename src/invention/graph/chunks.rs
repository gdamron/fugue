//! Feedback groups that run in chunks: when every cycle of a group passes
//! through a module declaring a minimum delay (see [`FeedbackCut`]), the
//! group runs in chunks of the smallest such delay instead of sample by
//! sample. Each chunk, the cut modules emit from their history, the other
//! members run block-wise in topological order, then the cut modules
//! absorb their inputs, so the loop is exactly as long as its delays.
//!
//! The plan is made when a topology compiles, on the control thread (a
//! live graph's compiles from the publisher's mirror and is swapped in with
//! the rest of the topology), and the main graph and developments share it.
//! The audio thread only reads it.

use std::sync::atomic::{AtomicU8, Ordering};

use super::ProcessGroup;
use crate::FeedbackCut;

/// The environment variable that turns chunked feedback off at process
/// start: `0`, `off` or `false`.
pub const FEEDBACK_CHUNKS_ENV: &str = "FUGUE_FEEDBACK_CHUNKS";

const UNSET: u8 = 0;
const ON: u8 = 1;
const OFF: u8 = 2;

static SWITCH: AtomicU8 = AtomicU8::new(UNSET);

/// Turns chunked feedback on or off for every topology defined from now
/// on (loads, reloads and edits, and developments as they are built),
/// overriding [`FEEDBACK_CHUNKS_ENV`]. Off, every feedback group runs
/// sample by sample, as it did before chunking. A topology already
/// defined keeps its scheduling until an edit changes it, even when it
/// compiles again for a new block size. Process-wide.
pub fn set_feedback_chunks(enabled: bool) {
    SWITCH.store(if enabled { ON } else { OFF }, Ordering::Relaxed);
}

/// Whether topologies compiled now run feedback through delays in chunks:
/// on unless [`set_feedback_chunks`] or [`FEEDBACK_CHUNKS_ENV`] turned it
/// off.
pub fn feedback_chunks() -> bool {
    let state = match SWITCH.load(Ordering::Relaxed) {
        UNSET => {
            let read = std::env::var(FEEDBACK_CHUNKS_ENV).ok();
            let from_env = if enabled_by(read.as_deref()) { ON } else { OFF };
            match SWITCH.compare_exchange(UNSET, from_env, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => from_env,
                Err(set) => set,
            }
        }
        state => state,
    };
    state == ON
}

/// Whether the environment's value leaves chunking on.
fn enabled_by(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    let value = value.trim();
    !(value == "0" || value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("false"))
}

/// How a feedback group runs in chunks.
#[derive(Debug, Clone)]
pub(crate) struct Chunks {
    /// Frames per chunk at most: the smallest delay a cut declares.
    pub(crate) frames: usize,
    /// The members declaring a cut, in member order: they emit before the
    /// rest of the chunk and absorb after it.
    pub(crate) cuts: Vec<usize>,
    /// For each cut, whether anything feeds an input it reads as it emits
    /// (from outside the group): only then are its inputs gathered for
    /// `emit` as well as for `absorb`.
    pub(crate) emit_reads: Vec<bool>,
    /// The other members, in an order where each follows every member it
    /// reads, once the routes into the cuts are left out.
    pub(crate) rest: Vec<usize>,
}

/// An edge between two modules, by index: a route into input port
/// `Some(port)`, or an ordering-only edge (`None`, see
/// [`crate::Module::control_targets`]). `from` past the last module is a
/// source outside them (a development's own input).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edge {
    pub(crate) from: usize,
    pub(crate) to: usize,
    pub(crate) port: Option<usize>,
}

/// Plans each feedback group of `groups` (`group_of` giving each module's
/// group) to run in chunks where it can, from `edges` and each module's
/// declared cut. Leaves every group sample by sample unless `chunked`.
/// Control thread: allocates.
pub(crate) fn plan_groups(
    chunked: bool,
    groups: &mut [ProcessGroup],
    group_of: &[usize],
    edges: impl IntoIterator<Item = Edge>,
    cut_of: impl Fn(usize) -> Option<FeedbackCut>,
) {
    if !chunked {
        return;
    }
    let mut inner: Vec<Vec<Edge>> = (0..groups.len()).map(|_| Vec::new()).collect();
    let mut emit_reads = vec![false; group_of.len()];
    for edge in edges {
        if let (Some(port), Some(cut)) = (edge.port, cut_of(edge.to)) {
            if !cut.absorbed.contains(&port) {
                emit_reads[edge.to] = true;
            }
        }
        let (Some(&from), Some(&to)) = (group_of.get(edge.from), group_of.get(edge.to)) else {
            continue;
        };
        if from == to && groups[to].feedback {
            inner[to].push(edge);
        }
    }
    for (group, edges) in groups.iter_mut().zip(&inner) {
        if group.feedback {
            group.chunks = plan(&group.members, edges, &cut_of).map(|mut chunks| {
                chunks.emit_reads = chunks.cuts.iter().map(|&m| emit_reads[m]).collect();
                chunks
            });
        }
    }
}

/// The chunks of a feedback group of `members` joined by `edges` (those
/// inside it), or `None` when it must run sample by sample: no member
/// declares a cut, a cut reads an input fed from inside the group while it
/// emits, a member writes another's controls (a chunk would apply the
/// write a chunk late or early, and where chunks start depends on the
/// block size), or a cycle passes through no cut.
fn plan(
    members: &[usize],
    edges: &[Edge],
    cut_of: impl Fn(usize) -> Option<FeedbackCut>,
) -> Option<Chunks> {
    let cuts: Vec<Option<FeedbackCut>> = members
        .iter()
        .map(|&m| cut_of(m).filter(|cut| cut.frames > 0))
        .collect();
    let at = |m: usize| members.iter().position(|&member| member == m);
    let frames = cuts.iter().flatten().map(|cut| cut.frames).min()?;
    for edge in edges {
        let port = edge.port?;
        if cuts[at(edge.to)?].is_some_and(|cut| !cut.absorbed.contains(&port)) {
            return None;
        }
    }

    // The rest in topological order, the routes into cuts left out; a
    // cycle left among them has no delay.
    let mut waiting: Vec<usize> = vec![0; members.len()];
    for edge in edges {
        let (from, to) = (at(edge.from)?, at(edge.to)?);
        if cuts[from].is_none() && cuts[to].is_none() {
            waiting[to] += 1;
        }
    }
    let rest_count = cuts.iter().filter(|cut| cut.is_none()).count();
    let mut placed = vec![false; members.len()];
    let mut rest = Vec::with_capacity(rest_count);
    while rest.len() < rest_count {
        let next =
            (0..members.len()).find(|&k| cuts[k].is_none() && !placed[k] && waiting[k] == 0)?;
        placed[next] = true;
        rest.push(members[next]);
        for edge in edges.iter().filter(|edge| edge.from == members[next]) {
            let to = at(edge.to)?;
            if cuts[to].is_none() {
                waiting[to] -= 1;
            }
        }
    }
    let cuts: Vec<usize> = members
        .iter()
        .zip(&cuts)
        .filter_map(|(&m, cut)| cut.map(|_| m))
        .collect();
    // Every cut gathers for emit until `plan_groups` knows its inputs.
    let emit_reads = vec![true; cuts.len()];
    Some(Chunks {
        frames,
        cuts,
        emit_reads,
        rest,
    })
}

#[cfg(test)]
mod tests;
