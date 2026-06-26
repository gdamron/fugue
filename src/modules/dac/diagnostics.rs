//! Lock-free audio callback diagnostics shared by native audio backends.

use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Number of cpal stream errors reported through the error callback.
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
    }
}
