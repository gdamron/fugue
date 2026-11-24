//! Acceptance tests for the views reading.
//!
//! `RenderEngine::take_view_reading(clock)` returns a `ViewReading`: the
//! output levels since the last read (as `take_levels` returns them, which
//! then start over) and the elapsed beats of the clock named, if the piece
//! has it. It is `None` with nothing loaded and allocation-free.
//!
//! What they check:
//! - no heap allocation or free on the calling thread, with an existing
//!   clock, a missing one and `None` (`no_alloc`);
//! - `levels` equal what `take_levels` returns at that point, and the next
//!   read starts over (`levels`);
//! - `beats` equals the clock's `elapsed` control (within f32 precision) for
//!   an existing clock and is `None` for a missing clock or `None` (`beats`);
//! - `take_levels` and `get_control` are unchanged (`existing_callers`).

mod beats;
mod existing_callers;
mod levels;
mod no_alloc;
mod support;
