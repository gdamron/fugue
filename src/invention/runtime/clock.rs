//! Anchoring the transport's wall clock from the audio callback.

use std::sync::Arc;
use std::time::Instant;

use crate::control_request::Transport;
use crate::modules::AudioDiagnostics;

/// Anchors a live stream's [`Transport`] once per device callback, from
/// the render function the backend calls (see
/// `InventionRuntime::start_with_backend`).
///
/// A backend may split one device callback into several render calls, all
/// within microseconds, while their frames are heard a whole buffer apart.
/// So with device timing (a backend's [`AudioDiagnostics`]) only the first
/// render call of each callback anchors, with the callback's start plus the
/// output latency the host reported; a backend without it (or whose
/// diagnostics record no callback times) anchors at every render call, with
/// no latency.
pub(crate) struct CallbackClock {
    device: Option<Arc<AudioDiagnostics>>,
    /// The start of the latest callback anchored.
    anchored: Option<Instant>,
}

impl CallbackClock {
    /// Starts `transport`'s wall clock at the backend's `sample_rate`,
    /// timed by `device` when the backend reports callback timing. Control
    /// thread, before the stream starts.
    pub(crate) fn start(
        transport: &Transport,
        sample_rate: u32,
        device: Option<Arc<AudioDiagnostics>>,
    ) -> Self {
        transport.start_clock(Instant::now(), sample_rate);
        Self {
            device,
            anchored: None,
        }
    }

    /// Anchors `transport` with the render call about to process `sample`,
    /// when it is the first of a device callback. Audio thread: wait-free,
    /// allocation- and lock-free.
    #[inline]
    pub(crate) fn anchor(&mut self, transport: &Transport, sample: u64) {
        if let Some(heard) = self.heard() {
            transport.anchor(sample, heard);
        }
    }

    /// When the render call starting now is heard, if it should anchor.
    fn heard(&mut self) -> Option<Instant> {
        let timing = self.device.as_ref().and_then(|d| d.callback_timing());
        let Some((started, latency)) = timing else {
            // No device timing (not every backend's diagnostics record it).
            return Some(Instant::now());
        };
        if self.anchored == Some(started) {
            return None;
        }
        self.anchored = Some(started);
        Some(started + latency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_counter::allocator_events;
    use std::time::Duration;

    #[test]
    fn only_the_first_render_call_of_a_callback_anchors_with_its_latency() {
        let device = Arc::new(AudioDiagnostics::new());
        let transport = Transport::new();
        let mut clock = CallbackClock::start(&transport, 48_000, Some(Arc::clone(&device)));
        let latency = Duration::from_millis(10);

        // No callback timing recorded yet: the render call itself anchors.
        let before = Instant::now();
        assert!(clock.heard().is_some_and(|heard| heard >= before));
        let first = Instant::now();
        device.record_callback_at(first);
        device.record_output_latency(latency);
        let (heard, allocs, frees) = allocator_events(|| clock.heard());
        assert_eq!((allocs, frees), (0, 0));
        let heard = heard.unwrap();
        let offset = heard.duration_since(first);
        assert!(
            offset.abs_diff(latency) < Duration::from_micros(1),
            "{offset:?}"
        );
        // A second render call in the same callback does not.
        assert_eq!(clock.heard(), None);

        device.record_callback_at(first + Duration::from_millis(5));
        assert!(clock.heard().unwrap() > heard);
    }

    #[test]
    fn without_device_timing_every_render_call_anchors_now() {
        let transport = Transport::new();
        let mut clock = CallbackClock::start(&transport, 48_000, None);
        let before = Instant::now();
        assert!(clock.heard().unwrap() >= before);
        assert!(clock.heard().is_some());
    }
}
