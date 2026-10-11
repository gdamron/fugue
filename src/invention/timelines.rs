//! Timeline context: the clock a module's beats count on when a request
//! timed in beats names none (B3; ledger `conditions-and-timeline-context`).
//!
//! Worked out on a control thread each time a topology is compiled for
//! publication, never on the audio thread. A front door resolves the clock
//! before it submits ([`Publisher::beat_clock`]), and the request carries it
//! as a named clock ([`BeatTime::clock`]). Later graph changes recompute the
//! context, but a pending request keeps the clock it resolved against, and
//! is refused (`TimelineGone`) if that clock goes away.
//!
//! # The rule
//!
//! - A module's timeline is the nearest clock upstream of it in signal flow.
//!   The walk goes back through connections from any of its inputs, and a
//!   path ends at the first clock on it. Distance counts connections walked.
//!   A clock's timeline is its own.
//! - Ties go to the clock first in module order.
//! - With no clock connected, the timeline is the first clock in module
//!   order (top-down). With no clock at all there is none, and an unnamed
//!   request is refused.
//!
//! [`Publisher::beat_clock`]: crate::invention::publish::Publisher::beat_clock
//! [`BeatTime::clock`]: crate::control_request::BeatTime

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use crate::Module;

/// How a module passes timing along, as the walk sees it.
#[derive(Clone, Debug, Default)]
pub(crate) enum Flow {
    #[default]
    Plain,
    /// Keeps a beat timeline: a clock.
    Clock,
}

impl Flow {
    /// Read from a built instance, on a control thread.
    pub(crate) fn of(module: &dyn Module) -> Self {
        if module.timeline().is_some() {
            Self::Clock
        } else {
            Self::Plain
        }
    }
}

/// A connection, by module and port index.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Link {
    pub(crate) from: usize,
    pub(crate) from_port: usize,
    pub(crate) to: usize,
    pub(crate) to_port: usize,
}

/// A module's timeline context, by module index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    /// Driven by `clock`, `distance` connections upstream.
    Connected { clock: usize, distance: u32 },
    /// No clock is connected: the first clock in module order.
    Fallback { clock: usize },
    /// The graph has no clock.
    None,
}

/// Every module's timeline context, in module order.
pub(crate) fn contexts(flows: &[Flow], links: &[Link]) -> Vec<Context> {
    let first = flows.iter().position(|flow| matches!(flow, Flow::Clock));
    (0..flows.len())
        .map(|module| match &flows[module] {
            Flow::Clock => Context::Connected {
                clock: module,
                distance: 0,
            },
            Flow::Plain => {
                let walk = walk(flows, links, Node::In(module, None));
                match walk.clocks.into_iter().min() {
                    Some((distance, clock)) => Context::Connected { clock, distance },
                    None => first.map_or(Context::None, |clock| Context::Fallback { clock }),
                }
            }
        })
        .collect()
}

/// Where a signal is, walking upstream: leaving a module's output port, or
/// entering a module (at one input port, or any).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Node {
    Out(usize, usize),
    In(usize, Option<usize>),
}

/// What a walk found: clocks as `(distance, module)`.
#[derive(Default)]
struct Walk {
    clocks: Vec<(u32, usize)>,
}

/// Walks upstream from `start` in order of distance, ending each path at
/// its first clock.
fn walk(flows: &[Flow], links: &[Link], start: Node) -> Walk {
    let mut queue = BinaryHeap::from([Reverse((0u32, start))]);
    let mut seen = HashSet::new();
    let mut found = Walk::default();
    while let Some(Reverse((distance, node))) = queue.pop() {
        if !seen.insert(node) {
            continue;
        }
        match node {
            Node::Out(module, _) => match &flows[module] {
                Flow::Clock => found.clocks.push((distance, module)),
                Flow::Plain => queue.push(Reverse((distance, Node::In(module, None)))),
            },
            Node::In(module, port) => {
                let enters = |to: usize, to_port: usize| {
                    to == module && port.is_none_or(|port| port == to_port)
                };
                for link in links.iter().filter(|link| enters(link.to, link.to_port)) {
                    let source = Node::Out(link.from, link.from_port);
                    queue.push(Reverse((distance.saturating_add(1), source)));
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(from: usize, to: usize) -> Link {
        Link {
            from,
            from_port: 0,
            to,
            to_port: 0,
        }
    }

    #[test]
    fn a_feedback_loop_ends_and_ties_go_to_module_order() {
        // 0 and 1 feed each other; clocks 3 and 2 both drive 4 directly.
        let flows = [
            Flow::Plain,
            Flow::Plain,
            Flow::Clock,
            Flow::Clock,
            Flow::Plain,
        ];
        let links = [link(0, 1), link(1, 0), link(3, 4), link(2, 4)];
        let found = contexts(&flows, &links);
        assert_eq!(found[0], Context::Fallback { clock: 2 });
        assert_eq!(found[1], Context::Fallback { clock: 2 });
        assert_eq!(
            found[4],
            Context::Connected {
                clock: 2,
                distance: 1
            }
        );
        assert_eq!(contexts(&[Flow::Plain], &[]), [Context::None]);
    }
}
