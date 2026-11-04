//! Acceptance tests for the `chance` module, written from its published
//! interface and acceptance criteria alone. Everything is addressed by
//! string: the module type id, port names, control names and invention JSON.
//!
//! The clock is driven by hand (see `rig`): every step is one clock pulse and
//! one block of four samples.

mod rig;

mod interface;
mod rhythm;
mod spread;
