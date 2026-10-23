//! Timeline context: the clock a module's beats count on when a request
//! timed in beats names none. See `docs/musical-time.md`, "Which clock".
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
//! - Developments are crossed. A development's output leads back only to
//!   the inputs that reach it inside, and the connections inside count
//!   towards the distance as if the development were laid out flat.
//! - A clock inside a development keeps a timeline that requests cannot
//!   name yet. When one is nearest, or the module is a development that
//!   holds one, the context is enclosed and an unnamed request is refused:
//!   no other clock is the one driving the module.
//! - Ties go to a clock requests can name over an enclosed one, then to the
//!   one first in module order.
//! - With no clock connected, the timeline is the first clock a request
//!   can name, in module order (top-down). When only developments hold
//!   clocks, it is the first of them, enclosed, and an unnamed request is
//!   refused. With no clock at all there is none, and it is refused too.
//!
//! [`Publisher::beat_clock`]: crate::invention::publish::Publisher::beat_clock
//! [`BeatTime::clock`]: crate::control_request::BeatTime

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::Arc;

use crate::Module;

/// How a module passes timing along, as the walk sees it.
#[derive(Clone, Debug, Default)]
pub(crate) enum Flow {
    #[default]
    Plain,
    /// Keeps a beat timeline: a clock.
    Clock,
    /// A development, seen from outside.
    Development(Arc<Enclosure>),
}

impl Flow {
    /// Read from a built instance, on a control thread.
    pub(crate) fn of(module: &dyn Module) -> Self {
        if module.timeline().is_some() {
            Self::Clock
        } else if let Some(enclosure) = module.enclosure() {
            Self::Development(Arc::clone(enclosure))
        } else {
            Self::Plain
        }
    }
}

/// A development's signal flow, as seen from outside it.
#[derive(Debug, Default)]
pub(crate) struct Enclosure {
    /// Whether it holds a clock, at any depth.
    pub(crate) clock: bool,
    /// What reaches each output port, by port index.
    pub(crate) outputs: Vec<Reach>,
}

/// What reaches one output of a development from inside it.
#[derive(Debug, Default)]
pub(crate) struct Reach {
    /// Distance to the nearest clock inside, when a path reaches one.
    pub(crate) clock: Option<u32>,
    /// Input ports whose signal reaches the output, each with its distance.
    pub(crate) inputs: Vec<(usize, u32)>,
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
    /// Driven by a clock inside `development`.
    Enclosed { development: usize },
    /// No clock is connected: the first clock a request can name.
    Fallback { clock: usize },
    /// The graph has no clock.
    None,
}

/// Every module's timeline context, in module order.
pub(crate) fn contexts(flows: &[Flow], links: &[Link]) -> Vec<Context> {
    let holds = |flow: &Flow| matches!(flow, Flow::Development(e) if e.clock);
    let fallback = match flows.iter().position(|flow| matches!(flow, Flow::Clock)) {
        Some(clock) => Context::Fallback { clock },
        None => match flows.iter().position(holds) {
            Some(development) => Context::Enclosed { development },
            None => Context::None,
        },
    };
    let incoming = incoming(flows.len(), links);
    (0..flows.len())
        .map(|module| match &flows[module] {
            Flow::Clock => Context::Connected {
                clock: module,
                distance: 0,
            },
            Flow::Development(enclosure) if enclosure.clock => Context::Enclosed {
                development: module,
            },
            _ => {
                let walk = walk(flows, &incoming, &[], Node::In(module, None));
                match walk.clocks.into_iter().min() {
                    Some((distance, false, clock)) => Context::Connected { clock, distance },
                    Some((_, true, development)) => Context::Enclosed { development },
                    None => fallback,
                }
            }
        })
        .collect()
}

/// The links into each module, by module index.
fn incoming(modules: usize, links: &[Link]) -> Vec<Vec<Link>> {
    let mut incoming = vec![Vec::new(); modules];
    for link in links {
        incoming[link.to].push(*link);
    }
    incoming
}

/// A development's flow, from its inner modules and connections.
/// `boundary` lists the inner `(module, port)` each input port index feeds;
/// `outputs` the inner `(module, port)` each output port index comes from.
pub(crate) fn enclosure(
    flows: &[Flow],
    links: &[Link],
    boundary: &[(usize, usize, usize)],
    outputs: impl Iterator<Item = Option<(usize, usize)>>,
) -> Enclosure {
    let clock = flows.iter().any(|flow| match flow {
        Flow::Clock => true,
        Flow::Development(enclosure) => enclosure.clock,
        Flow::Plain => false,
    });
    let incoming = incoming(flows.len(), links);
    let outputs = outputs
        .map(|source| {
            let Some((module, port)) = source else {
                return Reach::default();
            };
            let walk = walk(flows, &incoming, boundary, Node::Out(module, port));
            Reach {
                clock: walk.clocks.iter().map(|&(distance, ..)| distance).min(),
                inputs: walk.inputs,
            }
        })
        .collect();
    Enclosure { clock, outputs }
}

/// Where a signal is, walking upstream: leaving a module's output port, or
/// entering a module (at one input port, or any).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Node {
    Out(usize, usize),
    In(usize, Option<usize>),
}

/// What a walk found: clocks as `(distance, enclosed, module)`, and the
/// boundary inputs reached, each at its least distance.
#[derive(Default)]
struct Walk {
    clocks: Vec<(u32, bool, usize)>,
    inputs: Vec<(usize, u32)>,
}

/// Walks upstream from `start` in order of distance, ending each path at
/// its first clock.
fn walk(
    flows: &[Flow],
    incoming: &[Vec<Link>],
    boundary: &[(usize, usize, usize)],
    start: Node,
) -> Walk {
    let mut queue = BinaryHeap::from([Reverse((0u32, start))]);
    let mut seen = HashSet::new();
    let mut found = Walk::default();
    while let Some(Reverse((distance, node))) = queue.pop() {
        if !seen.insert(node) {
            continue;
        }
        match node {
            Node::Out(module, port) => match &flows[module] {
                Flow::Clock => found.clocks.push((distance, false, module)),
                Flow::Plain => queue.push(Reverse((distance, Node::In(module, None)))),
                Flow::Development(enclosure) => {
                    let Some(reach) = enclosure.outputs.get(port) else {
                        continue;
                    };
                    if let Some(inner) = reach.clock {
                        found
                            .clocks
                            .push((distance.saturating_add(inner), true, module));
                    }
                    for &(input, inner) in &reach.inputs {
                        let entering = Node::In(module, Some(input));
                        queue.push(Reverse((distance.saturating_add(inner), entering)));
                    }
                }
            },
            Node::In(module, port) => {
                let enters = |to_port: usize| port.is_none_or(|port| port == to_port);
                for link in incoming[module].iter().filter(|link| enters(link.to_port)) {
                    let source = Node::Out(link.from, link.from_port);
                    queue.push(Reverse((distance.saturating_add(1), source)));
                }
                for &(to, to_port, input) in boundary {
                    if to == module
                        && enters(to_port)
                        && found.inputs.iter().all(|&(i, _)| i != input)
                    {
                        found.inputs.push((input, distance));
                    }
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
        // 0 and 1 feed each other; clocks 3 and 2 both drive 4 directly,
        // 3's link listed first: the tie still goes to 2, first in order.
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

    #[test]
    fn a_clock_a_request_can_name_wins_a_tie_with_an_enclosed_one() {
        // A development whose output comes straight from its inner clock,
        // first in module order, and a clock: both drive 2 directly.
        let enclosure = Enclosure {
            clock: true,
            outputs: vec![Reach {
                clock: Some(0),
                inputs: Vec::new(),
            }],
        };
        let flows = [
            Flow::Development(Arc::new(enclosure)),
            Flow::Clock,
            Flow::Plain,
        ];
        let found = contexts(&flows, &[link(0, 2), link(1, 2)]);
        let on_clock = Context::Connected {
            clock: 1,
            distance: 1,
        };
        assert_eq!(found[2], on_clock);
        // Without it, the development's clock drives 2, and is reported.
        let found = contexts(
            &flows[..1]
                .iter()
                .cloned()
                .chain([Flow::Plain, Flow::Plain])
                .collect::<Vec<_>>(),
            &[link(0, 2)],
        );
        assert_eq!(found[2], Context::Enclosed { development: 0 });
        // Only an enclosed clock anywhere: an unconnected module reports it.
        assert_eq!(found[1], Context::Enclosed { development: 0 });
    }
}
