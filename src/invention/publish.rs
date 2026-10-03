//! Atomic, allocation-free structural changes to a live graph.
//!
//! A structural change to a running invention goes through three steps:
//!
//! 1. **Prepare** (control thread, all fallible work): a [`GraphChange`]
//!    builds instances, validates edits, attaches schedulers, and compiles
//!    the complete next topology. Nothing visible changes.
//! 2. **Publish**: the prepared topology goes to the audio thread as one
//!    publication, installed at the start of one block without allocating,
//!    freeing, or locking (see `graph::publication`). The old structures come
//!    back to the control thread to be freed.
//! 3. **Commit** (control thread): the runtime's mirrors (modules,
//!    connections, ports, control surfaces, document) update together.
//!
//! Offline render owns its graph outright and keeps applying edits directly.

mod change;
