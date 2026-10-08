//! The sample transport: the first layer of musical time.
//!
//! The engine counts samples since its graph was created: Fugue always runs
//! from load, with no transport start or stop, and the count never goes
//! back. The audio thread owns the count (`SignalGraph::current_sample`)
//! and publishes it here after every block; control threads read it to
//! time requests (see [`RequestSender::submit`](super::RequestSender::submit)).
//! There is no beat or tempo at this layer: clock timelines build on it.
//!
//! Offline render runs the same `process_block` over the same counter, so a
//! request timed in samples lands on the same sample there as live.
//!
//! # Wall-clock times
//!
//! A live stream also gives the transport a wall clock
//! ([`Transport::start_clock`]): at every device callback the audio thread
//! anchors it with the sample the callback starts at and when that sample
//! will be heard (the callback's start plus the output latency the host
//! reports). A sample's time follows from the latest anchor at the nominal
//! sample rate, so one word holds the whole relation: when sample 0 would be
//! heard. Control threads convert a wall-clock time to the sample heard
//! then ([`Transport::sample_at`]). Offline render has no wall clock.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::sync::{AtomicU64, Ordering};

const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// The engine's published sample count. Shared between the audio thread,
/// its only writer, and any number of control-thread readers.
///
/// One atomic word with one writer, so `Relaxed` is enough on both sides:
/// per-location coherence means no reader ever sees the count go back, and
/// nothing else is published through it (a request carries its own data
/// through the queue, which orders it).
pub(crate) struct Transport {
    /// Samples the audio thread has rendered, which is also the sample the
    /// next block starts at.
    rendered: AtomicU64,
    /// Set once, before a live stream starts; read without locking.
    clock: OnceLock<WallClock>,
}

/// The relation between samples and wall-clock time.
struct WallClock {
    epoch: Instant,
    sample_rate: u32,
    /// When sample 0 is heard, in nanoseconds after `epoch` (an `i64`'s
    /// bits; negative when that was before `epoch`), as of the latest
    /// anchor. One word, so `Relaxed` publishes it whole.
    heard_zero: AtomicU64,
}

impl WallClock {
    /// When sample 0 is heard, if `sample` is heard at `heard`.
    fn zero(&self, sample: u64, heard: Instant) -> i64 {
        let heard = i128::try_from(heard.saturating_duration_since(self.epoch).as_nanos())
            .unwrap_or(i128::MAX);
        let since_zero = i128::from(sample) * NANOS_PER_SECOND / i128::from(self.sample_rate);
        (heard - since_zero).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
    }
}

impl Transport {
    pub(crate) fn new() -> Self {
        Self {
            rendered: AtomicU64::new(0),
            clock: OnceLock::new(),
        }
    }

    /// Gives a live stream's transport its wall clock, before the audio
    /// thread runs: until the first callback anchors it, the next sample is
    /// taken to be heard at `epoch`. A clock already started stays. Control
    /// thread.
    pub(crate) fn start_clock(&self, epoch: Instant, sample_rate: u32) {
        let clock = WallClock {
            epoch,
            sample_rate: sample_rate.max(1),
            heard_zero: AtomicU64::new(0),
        };
        let zero = clock.zero(self.rendered(), epoch);
        clock.heard_zero.store(zero as u64, Ordering::Relaxed);
        let _ = self.clock.set(clock);
    }

    /// Anchors the wall clock: `sample` is heard at `heard`. Audio thread,
    /// once per device callback: wait-free, allocation- and lock-free. Does
    /// nothing without a clock.
    #[inline]
    pub(crate) fn anchor(&self, sample: u64, heard: Instant) {
        if let Some(clock) = self.clock.get() {
            let zero = clock.zero(sample, heard);
            clock.heard_zero.store(zero as u64, Ordering::Relaxed);
        }
    }

    /// The sample heard at `at`, from the latest anchor: `None` without a
    /// wall clock (offline render). A time before sample 0 gives 0.
    ///
    /// # Error bound
    ///
    /// Against the sample a listener actually hears at `at`, the result is
    /// off by at most `1 + sample_rate * (delay + latency_error + lead *
    /// drift)` samples, where:
    ///
    /// - `delay` is how long after the host's callback timestamp the
    ///   callback reads the clock (microseconds);
    /// - `latency_error` is the error of the output latency the host
    ///   reports (exact on CoreAudio; an estimate on some Linux hosts);
    /// - `lead` is how far `at` is from the latest anchor, and `drift` the
    ///   rate difference between the device's clock and the system's
    ///   (typically under 100 ppm, so 1 ms over a 10 s lead at most);
    /// - the 1 is rounding.
    ///
    /// A backend without device timing anchors at each render call with no
    /// latency, so the result can also be early by its whole output
    /// latency. While the stream is stopped (its device gone, say) nothing
    /// anchors and the count stands still, so a time converted then is
    /// placed as if the stream had kept running: it is heard later than
    /// asked by however long the stream stopped.
    pub(crate) fn sample_at(&self, at: Instant) -> Option<u64> {
        let clock = self.clock.get()?;
        let at = match at.checked_duration_since(clock.epoch) {
            Some(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
            None => -i128::try_from(clock.epoch.duration_since(at).as_nanos()).unwrap_or(i128::MAX),
        };
        let since_zero = at - i128::from(clock.heard_zero.load(Ordering::Relaxed) as i64);
        let rate = i128::from(clock.sample_rate);
        let sample = (since_zero * rate + NANOS_PER_SECOND / 2).div_euclid(NANOS_PER_SECOND);
        Some(sample.clamp(0, i128::from(u64::MAX)) as u64)
    }

    /// `duration` in samples at the wall clock's rate, rounded: for a ttl
    /// given in time. `None` without a wall clock.
    pub(crate) fn samples_in(&self, duration: Duration) -> Option<u64> {
        let rate = u128::from(self.clock.get()?.sample_rate);
        let samples = (duration.as_nanos() * rate + 500_000_000) / 1_000_000_000;
        Some(u64::try_from(samples).unwrap_or(u64::MAX))
    }

    /// Publishes the count after a block. Audio thread only: wait-free,
    /// allocation- and lock-free.
    #[inline]
    pub(crate) fn publish(&self, rendered: u64) {
        self.rendered.store(rendered, Ordering::Relaxed);
    }

    /// Samples rendered as of the latest block the audio thread finished:
    /// the earliest sample a request submitted now can apply at. The audio
    /// thread may be part way into the next block, so a request timed close
    /// to this can still arrive after its sample and apply late.
    #[inline]
    pub(crate) fn rendered(&self) -> u64 {
        self.rendered.load(Ordering::Relaxed)
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
