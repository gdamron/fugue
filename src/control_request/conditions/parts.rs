//! How a waiting request's control and chained parts start, and how they
//! are met (see the parent's docs); signal parts read in the parent.

use super::{ConditionHost, Entry, Seen};
use crate::control_request::chains::Chains;
use crate::control_request::condition::Predicate;
use crate::control_request::pending::Outcome;

/// Starts `entry`'s parts reading from `now`: each signal part from its
/// output's last value, each control part from its control's value, and
/// each part waiting on a request watching it. `false` when one reads an
/// output or control its module does not have.
pub(super) fn arm(
    entry: &mut Entry,
    now: u64,
    host: &impl ConditionHost,
    chains: &mut Chains,
) -> bool {
    let parts = entry.condition.parts();
    for (part, seen) in parts.iter().zip(&mut entry.seen) {
        let last = match *part {
            Predicate::Signal { module, port, .. } => {
                host.last_output(module as usize, usize::from(port))
            }
            Predicate::Control {
                module, control, ..
            } => host.control(module as usize, control),
            Predicate::Applied(_) => Some(0.0),
        };
        let Some(last) = last else {
            return false;
        };
        let remaining = match *part {
            Predicate::Signal { count, .. } => count,
            _ => 1,
        };
        *seen = Seen {
            met: None,
            remaining,
            last,
            next: now,
        };
    }
    for part in parts {
        if let Predicate::Applied(id) = *part {
            chains.watch(id);
        }
    }
    true
}

/// Reads the controls `entry`'s parts wait on, at the segment start `now`.
pub(super) fn read_controls(entry: &mut Entry, now: u64, host: &impl ConditionHost) {
    let parts = entry.condition.parts();
    for (part, seen) in parts.iter().zip(&mut entry.seen) {
        let Predicate::Control {
            module,
            control,
            edge,
            level,
        } = *part
        else {
            continue;
        };
        let Some(value) = host.control(module as usize, control) else {
            continue;
        };
        if seen.met.is_none() && edge.crosses(seen.last, value, level) {
            seen.met = Some(now);
        }
        seen.last = value;
    }
}

/// Takes in what became of the requests `entry`'s parts wait on: met at
/// the sample one applied at, or broken when it settled otherwise.
pub(super) fn latch(entry: &mut Entry, chains: &Chains) {
    let parts = entry.condition.parts();
    for (part, seen) in parts.iter().zip(&mut entry.seen) {
        let Predicate::Applied(id) = *part else {
            continue;
        };
        match chains.outcome(id) {
            _ if seen.met.is_some() => {}
            None => {}
            Some(Outcome::Applied { at } | Outcome::AppliedLate { at, .. }) => seen.met = Some(at),
            Some(_) => entry.broken = true,
        }
    }
}
