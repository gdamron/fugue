//! How a waiting request's parts start, and how its control parts are met
//! (see the parent's docs); signal parts read in the parent.

use super::{ConditionHost, Entry, Seen};
use crate::control_request::condition::Predicate;

/// Starts `entry`'s parts reading from `now`: each signal part from its
/// output's last value, and each control part from its control's value.
/// `false` when one reads an output or control its module does not have.
pub(super) fn arm(entry: &mut Entry, now: u64, host: &impl ConditionHost) -> bool {
    let parts = entry.condition.parts();
    for (part, seen) in parts.iter().zip(&mut entry.seen) {
        let last = match *part {
            Predicate::Signal { module, port, .. } => {
                host.last_output(module as usize, usize::from(port))
            }
            Predicate::Control {
                module, control, ..
            } => host.control(module as usize, control),
            // Refused at submit (see `Conditions::check`).
            Predicate::Applied(_) => None,
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
