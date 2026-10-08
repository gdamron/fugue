//! Lock-free audio callback diagnostics shared by native audio backends.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const HISTOGRAM_BUCKET_NS: [u64; 12] = [
    125_000,
    250_000,
    500_000,
    1_000_000,
    2_000_000,
    4_000_000,
    8_000_000,
    16_000_000,
    32_000_000,
    64_000_000,
    128_000_000,
    u64::MAX,
];

/// Serializable point-in-time view of native audio callback timing.
///
/// Values are cumulative for the lifetime of the backend instance. Timing
/// fields are reported in milliseconds for status/RPC consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct AudioDiagnosticsSnapshot {
    /// Number of device callbacks observed by the backend.
    pub callback_count: u64,
    /// Number of buffer underruns/overruns the audio host reported.
    pub xrun_count: u64,
    /// Number of callbacks whose measured render time exceeded the buffer period.
    pub missed_deadline_count: u64,
    /// Sum of all measured callback durations.
    pub total_callback_ms: f64,
    /// Mean callback duration, or zero before the first callback.
    pub average_callback_ms: f64,
    /// Longest measured callback duration.
    pub max_callback_ms: f64,
    /// Coarse p99 callback duration estimated from fixed histogram buckets.
    pub p99_callback_ms: f64,
    /// Current device buffer period derived from callback frames and sample rate.
    pub buffer_period_ms: f64,
    /// Number of stream errors other than xruns, such as device loss or an
    /// invalidated stream.
    #[serde(default)]
    pub stream_error_count: u64,
    /// Kind of the most recent non-xrun stream error, if any.
    #[serde(default)]
    pub last_stream_error: Option<StreamErrorKind>,
    /// Time since the device last asked for audio, or `None` before the first
    /// callback. While a stream plays this stays near `buffer_period_ms`; a
    /// value growing well past it means the stream has stopped, whatever the
    /// cause.
    #[serde(default)]
    pub last_callback_age_ms: Option<f64>,
    /// Number of times the output stream was rebuilt after the host stopped
    /// it, e.g. because the output device went away.
    #[serde(default)]
    pub stream_restart_count: u64,
}

/// Why the audio host reported a stream error other than an xrun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StreamErrorKind {
    /// The output device went away and the stream stopped. The driver
    /// rebuilds it on the default output as soon as one is available.
    DeviceNotAvailable,
    /// The system default output moved to another device; the stream follows it.
    DeviceChanged,
    /// The stream configuration no longer matches the device, e.g. after a
    /// sample-rate change on some hosts. The stream stopped; the driver
    /// rebuilds it.
    StreamInvalidated,
    /// The host refused real-time scheduling for the audio thread.
    RealtimeDenied,
    /// Any other host error.
    Other,
}

impl StreamErrorKind {
    /// Whether the host stops the stream on this error, so it must be rebuilt.
    pub fn stops_stream(self) -> bool {
        matches!(self, Self::DeviceNotAvailable | Self::StreamInvalidated)
    }

    const ALL: [Self; 5] = [
        Self::DeviceNotAvailable,
        Self::DeviceChanged,
        Self::StreamInvalidated,
        Self::RealtimeDenied,
        Self::Other,
    ];

    /// Nonzero code for atomic storage; zero means no error.
    fn code(self) -> u8 {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0) as u8 + 1
    }

    fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code).checked_sub(1)?).copied()
    }
}

/// Lock-free accumulator for native audio callback diagnostics.
///
/// The audio thread only performs relaxed atomic updates against preallocated
/// counters. Status/RPC callers read snapshots from non-audio threads.
pub struct AudioDiagnostics {
    callback_count: AtomicU64,
    xrun_count: AtomicU64,
    missed_deadline_count: AtomicU64,
    total_callback_ns: AtomicU64,
    max_callback_ns: AtomicU64,
    buffer_period_ns: AtomicU64,
    histogram: [AtomicU64; HISTOGRAM_BUCKET_NS.len()],
    stream_error_count: AtomicU64,
    last_stream_error: AtomicU8,
    stream_restart_count: AtomicU64,
    /// Reference point for `last_callback_ns`.
    epoch: Instant,
    /// Nanoseconds from `epoch` to the latest callback, plus one; zero means
    /// no callback yet.
    last_callback_ns: AtomicU64,
    /// The output latency the host reported for the latest callback: how
    /// long after the callback its first frame is played.
    output_latency_ns: AtomicU64,
}

impl AudioDiagnostics {
    pub fn new() -> Self {
        Self {
            callback_count: AtomicU64::new(0),
            xrun_count: AtomicU64::new(0),
            missed_deadline_count: AtomicU64::new(0),
            total_callback_ns: AtomicU64::new(0),
            max_callback_ns: AtomicU64::new(0),
            buffer_period_ns: AtomicU64::new(0),
            histogram: std::array::from_fn(|_| AtomicU64::new(0)),
            stream_error_count: AtomicU64::new(0),
            last_stream_error: AtomicU8::new(0),
            stream_restart_count: AtomicU64::new(0),
            epoch: Instant::now(),
            last_callback_ns: AtomicU64::new(0),
            output_latency_ns: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn record_callback(&self, callback_ns: u64, buffer_period_ns: u64) -> bool {
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        self.total_callback_ns
            .fetch_add(callback_ns, Ordering::Relaxed);
        self.buffer_period_ns
            .store(buffer_period_ns, Ordering::Relaxed);
        self.record_max_callback(callback_ns);
        self.histogram[histogram_bucket(callback_ns)].fetch_add(1, Ordering::Relaxed);

        let missed_deadline = buffer_period_ns > 0 && callback_ns > buffer_period_ns;
        if missed_deadline {
            self.missed_deadline_count.fetch_add(1, Ordering::Relaxed);
        }
        missed_deadline
    }

    #[inline]
    pub fn record_xrun(&self) {
        self.xrun_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a stream error other than an xrun.
    #[inline]
    pub fn record_stream_error(&self, kind: StreamErrorKind) {
        self.stream_error_count.fetch_add(1, Ordering::Relaxed);
        self.last_stream_error.store(kind.code(), Ordering::Relaxed);
    }

    /// Records that the output stream was rebuilt.
    pub fn record_stream_restart(&self) {
        self.stream_restart_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks that the device asked for audio at `at`.
    #[inline]
    pub fn record_callback_at(&self, at: Instant) {
        let ns = at.saturating_duration_since(self.epoch).as_nanos();
        let ns = ns.min(u128::from(u64::MAX - 1)) as u64;
        self.last_callback_ns.store(ns + 1, Ordering::Relaxed);
    }

    /// Records the output latency the host reported for the callback just
    /// marked by [`Self::record_callback_at`].
    #[inline]
    pub fn record_output_latency(&self, latency: Duration) {
        let ns = latency.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.output_latency_ns.store(ns, Ordering::Relaxed);
    }

    /// When the latest callback started and its reported output latency,
    /// or `None` before the first. Read on the callback's own thread, as the
    /// transport's wall clock does, it is that callback's.
    pub(crate) fn callback_timing(&self) -> Option<(Instant, Duration)> {
        let ns = self
            .last_callback_ns
            .load(Ordering::Relaxed)
            .checked_sub(1)?;
        let latency = self.output_latency_ns.load(Ordering::Relaxed);
        Some((
            self.epoch + Duration::from_nanos(ns),
            Duration::from_nanos(latency),
        ))
    }

    fn last_callback_age_ns(&self, now: Instant) -> Option<u64> {
        let stored = self
            .last_callback_ns
            .load(Ordering::Relaxed)
            .checked_sub(1)?;
        let now_ns = now.saturating_duration_since(self.epoch).as_nanos();
        let now_ns = now_ns.min(u128::from(u64::MAX)) as u64;
        Some(now_ns.saturating_sub(stored))
    }

    pub fn snapshot(&self) -> AudioDiagnosticsSnapshot {
        let callback_count = self.callback_count.load(Ordering::Relaxed);
        let total_callback_ns = self.total_callback_ns.load(Ordering::Relaxed);
        let max_callback_ns = self.max_callback_ns.load(Ordering::Relaxed);
        let average_callback_ns = if callback_count == 0 {
            0.0
        } else {
            total_callback_ns as f64 / callback_count as f64
        };

        AudioDiagnosticsSnapshot {
            callback_count,
            xrun_count: self.xrun_count.load(Ordering::Relaxed),
            missed_deadline_count: self.missed_deadline_count.load(Ordering::Relaxed),
            total_callback_ms: ns_to_ms(total_callback_ns as f64),
            average_callback_ms: ns_to_ms(average_callback_ns),
            max_callback_ms: ns_to_ms(max_callback_ns as f64),
            p99_callback_ms: ns_to_ms(self.p99_callback_ns(callback_count, max_callback_ns) as f64),
            buffer_period_ms: ns_to_ms(self.buffer_period_ns.load(Ordering::Relaxed) as f64),
            stream_error_count: self.stream_error_count.load(Ordering::Relaxed),
            last_stream_error: StreamErrorKind::from_code(
                self.last_stream_error.load(Ordering::Relaxed),
            ),
            last_callback_age_ms: self
                .last_callback_age_ns(Instant::now())
                .map(|ns| ns_to_ms(ns as f64)),
            stream_restart_count: self.stream_restart_count.load(Ordering::Relaxed),
        }
    }

    fn record_max_callback(&self, callback_ns: u64) {
        let mut current = self.max_callback_ns.load(Ordering::Relaxed);
        while callback_ns > current {
            match self.max_callback_ns.compare_exchange_weak(
                current,
                callback_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }

    fn p99_callback_ns(&self, callback_count: u64, max_callback_ns: u64) -> u64 {
        if callback_count == 0 {
            return 0;
        }

        let target = callback_count.saturating_mul(99).saturating_add(99) / 100;
        let mut cumulative = 0u64;
        for (index, bucket) in self.histogram.iter().enumerate() {
            cumulative = cumulative.saturating_add(bucket.load(Ordering::Relaxed));
            if cumulative >= target {
                let upper_bound = HISTOGRAM_BUCKET_NS[index];
                return if upper_bound == u64::MAX {
                    max_callback_ns
                } else {
                    upper_bound
                };
            }
        }

        max_callback_ns
    }
}

impl Default for AudioDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn histogram_bucket(callback_ns: u64) -> usize {
    HISTOGRAM_BUCKET_NS
        .iter()
        .position(|upper_bound| callback_ns <= *upper_bound)
        .unwrap_or(HISTOGRAM_BUCKET_NS.len() - 1)
}

#[inline]
fn ns_to_ms(ns: f64) -> f64 {
    ns / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reports_counts_and_timing() {
        let diagnostics = AudioDiagnostics::new();

        assert!(!diagnostics.record_callback(250_000, 1_000_000));
        assert!(diagnostics.record_callback(2_500_000, 1_000_000));
        diagnostics.record_xrun();

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.callback_count, 2);
        assert_eq!(snapshot.xrun_count, 1);
        assert_eq!(snapshot.missed_deadline_count, 1);
        assert_eq!(snapshot.total_callback_ms, 2.75);
        assert_eq!(snapshot.average_callback_ms, 1.375);
        assert_eq!(snapshot.max_callback_ms, 2.5);
        assert_eq!(snapshot.p99_callback_ms, 4.0);
        assert_eq!(snapshot.buffer_period_ms, 1.0);
    }

    #[test]
    fn p99_returns_zero_without_callbacks() {
        let snapshot = AudioDiagnostics::new().snapshot();

        assert_eq!(snapshot.callback_count, 0);
        assert_eq!(snapshot.p99_callback_ms, 0.0);
        assert_eq!(snapshot.last_callback_age_ms, None);
    }

    #[test]
    fn stream_errors_are_counted_apart_from_xruns() {
        let diagnostics = AudioDiagnostics::new();
        diagnostics.record_xrun();
        diagnostics.record_stream_error(StreamErrorKind::DeviceChanged);
        diagnostics.record_stream_error(StreamErrorKind::StreamInvalidated);

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.xrun_count, 1);
        assert_eq!(snapshot.stream_error_count, 2);
        assert_eq!(
            snapshot.last_stream_error,
            Some(StreamErrorKind::StreamInvalidated)
        );
    }

    #[test]
    fn stream_error_kinds_round_trip_through_codes() {
        assert_eq!(StreamErrorKind::from_code(0), None);
        for kind in StreamErrorKind::ALL {
            assert_eq!(StreamErrorKind::from_code(kind.code()), Some(kind));
        }
    }

    #[test]
    fn callback_age_grows_after_the_last_callback() {
        let diagnostics = AudioDiagnostics::new();
        let at = diagnostics.epoch + std::time::Duration::from_millis(5);
        diagnostics.record_callback_at(at);

        let later = at + std::time::Duration::from_millis(250);
        assert_eq!(diagnostics.last_callback_age_ns(at), Some(0));
        assert_eq!(diagnostics.last_callback_age_ns(later), Some(250_000_000));
    }

    #[test]
    fn snapshots_without_new_fields_still_deserialize() {
        let json = r#"{"callback_count":1,"xrun_count":0,"missed_deadline_count":0,
            "total_callback_ms":1.0,"average_callback_ms":1.0,"max_callback_ms":1.0,
            "p99_callback_ms":1.0,"buffer_period_ms":10.0}"#;
        let snapshot: AudioDiagnosticsSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.stream_error_count, 0);
        assert_eq!(snapshot.last_stream_error, None);
        assert_eq!(snapshot.last_callback_age_ms, None);
        assert_eq!(snapshot.stream_restart_count, 0);
    }
}
