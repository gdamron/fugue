//! An inner module panicking while a development hands it a payload: the
//! wrapper the development unwrapped, and what other aliases replaced, are
//! set aside by the unwind for the graph to keep, not freed on the
//! rendering thread nor leaked.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use super::fan_out::{apply_fanned_out, fanned_out};
use super::retarget::{apply_reordered, reordered};
use crate::alloc_counter::foreign_frees;
use crate::payload::{take_caught, Payload, RenderScope, Retired};

/// Runs `apply` as a render would, which must panic, counting the frees it
/// makes of memory allocated before it, and how many values it set aside.
fn panicking_render<R>(apply: impl FnOnce() -> R) -> (usize, usize) {
    let (applied, foreign) = foreign_frees(|| {
        catch_unwind(AssertUnwindSafe(|| {
            let _rendering = RenderScope::enter();
            apply()
        }))
    });
    assert!(applied.is_err(), "the inner module panics");
    let caught = take_caught().into_iter().flatten().count();
    (foreign, caught)
}

/// An inner module that panics, as a unit so the unwind allocates nothing.
fn panics(_: Payload) -> Result<Retired, (crate::control_request::Refusal, Payload)> {
    resume_unwind(Box::new(()))
}

#[test]
fn a_panic_applying_a_reordered_payload_frees_nothing() {
    let payload = reordered(Payload::new(1u32), vec![1, 0].into_boxed_slice());
    let mut order = [0, 1];
    let (frees, caught) = panicking_render(|| apply_reordered(&mut order, payload, panics));
    assert_eq!(frees, 0, "freed on the rendering thread");
    // The inner payload, then the box it came in.
    assert_eq!(caught, 2, "set aside for the graph");
}

#[test]
fn a_panic_in_a_later_alias_frees_nothing_the_earlier_ones_replaced() {
    let payload = fanned_out(vec![Payload::new(1u32), Payload::new(2u32)]);
    let (frees, caught) = panicking_render(|| {
        apply_fanned_out(2, payload, |position, payload| match position {
            0 => Ok(Retired::from(payload)),
            _ => panics(payload),
        })
    });
    assert_eq!(frees, 0, "freed on the rendering thread");
    // The later alias's payload, then the box holding what the earlier
    // one replaced.
    assert_eq!(caught, 2, "set aside for the graph");
}
