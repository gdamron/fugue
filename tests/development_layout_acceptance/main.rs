//! Acceptance tests for arranging the inside of a development the piece owns,
//! written from the spec alone and driving only public boundaries: a whole
//! invention running on the live engine (`RunningInvention::apply_edits`,
//! `development`, `document`), the files on disk, and the wire types.
//!
//! Criteria:
//! - a move inside an inline development is saved in the invention's own
//!   document as that definition's `layout`, readable back and through a save
//!   (`inline`), nested paths too;
//! - a move inside a file development rewrites only that file's `layout`:
//!   every other parsed member equal and every other byte identical, its
//!   permissions kept (`file`);
//! - a move changes nothing heard: the output is sample-identical to an
//!   unmoved run and nothing is rebuilt (`audio`);
//! - every use shows the new arrangement: two instances, and one file
//!   declared under two names (`file`);
//! - an installed-package development, and a file in the package cache, stay
//!   read-only and say so (`package`);
//! - a file that cannot be written, or that changed on disk, is refused with
//!   a reason and nothing changes (`refusals`);
//! - a move inside a development is applied alone and at once: mixed with a
//!   structural edit, timed, or writing two files, it is refused with nothing
//!   applied (`refusals`);
//! - the edit is the agent's: it parses from the wire and repeats to the same
//!   result (`inline`, `wire`).

mod harness;
mod inline;
mod package;

pub use harness::*;
pub use serde_json::{json, Value};
