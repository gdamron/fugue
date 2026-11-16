//! The cell sequencer's gate bridged into a held step, when the clock stops
//! and resumes, through the public API (the `CellSequencer` module driven
//! sample by sample, the `Clock` module, and `RenderEngine`):
//!
//! - A note followed by a held step, whose clock stops after the note's
//!   edge, releases its gate two measured step durations plus two samples
//!   after the last edge, and stays low until the clock resumes. The same
//!   holds when the clock stops during a held step followed by another held
//!   step, and for a principal deferred behind an on-beat grace chain.
//! - With a running clock nothing changes: a note and its held steps are
//!   one unbroken gate, through a held edge up to two measured steps (plus
//!   two samples) late, and through a fractional tempo halving.
//! - When the clock resumes after a release, a held step continuing the
//!   note re-attacks it at that edge, a held step with no note sounding is
//!   silent, a rest is low and a note gives a fresh rising edge.

mod rig;

mod release;
mod resume;
mod running;
