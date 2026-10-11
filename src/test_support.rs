//! Helpers shared by tests that wait on background threads.

pub(crate) mod dial;
pub(crate) mod tape;

use std::thread;
use std::time::{Duration, Instant};

/// Upper bound for a test waiting on a background thread: an audio worker,
/// a script thread, or a fake ffmpeg child process.
///
/// On a heavily loaded machine those can take seconds to get scheduled or
/// start (a fake ffmpeg starts a `python3` interpreter). Waits poll the real
/// condition and return as soon as it holds, so this bound only matters when
/// a test is about to fail anyway.
pub(crate) const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Polls `condition` until it holds or [`WAIT_TIMEOUT`] elapses, returning
/// whether it held.
pub(crate) fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
