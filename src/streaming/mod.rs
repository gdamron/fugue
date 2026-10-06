//! Shared streaming backends used by sink/tap modules.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod ffmpeg;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod video;

#[cfg(test)]
pub(crate) mod test_support {
    use std::thread;
    use std::time::{Duration, Instant};

    /// Upper bound for tests waiting on a streaming worker.
    ///
    /// The fake ffmpeg fixtures start a `python3` interpreter, which can take
    /// seconds on a heavily loaded machine. Waits poll the real condition and
    /// return as soon as it holds, so this bound only matters when a test is
    /// about to fail anyway.
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
}
