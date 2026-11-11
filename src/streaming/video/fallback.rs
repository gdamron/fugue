//! Paced black frames for a stream with no picture of its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::clock::{wait_on, ClockWaiter, VideoClock, WorkerExit};
use super::VideoFrameTarget;

/// Emits paced black RGBA frames until an external video producer takes over.
///
/// The frame is allocated once on the worker thread's setup path. The audio
/// thread never interacts with this source.
#[derive(Clone)]
pub(crate) struct BlackVideoFallbackHandle {
    shared: Arc<BlackVideoFallbackShared>,
}

pub(crate) struct BlackVideoFallback;

struct BlackVideoFallbackShared {
    stopping: AtomicBool,
    external_video: AtomicBool,
    /// Held while the worker checks its state and waits on `wake`.
    wake_lock: Mutex<()>,
    wake: Condvar,
    join_handle: Mutex<Option<JoinHandle<()>>>,
    last_error: Mutex<Option<String>>,
}

impl ClockWaiter for BlackVideoFallbackShared {
    fn clock_advanced(&self) {
        let _guard = self.wake_lock.lock().unwrap();
        self.wake.notify_all();
    }
}

impl BlackVideoFallback {
    /// Starts the fallback paced by `clock`.
    pub(crate) fn start_with_clock(
        width: u32,
        height: u32,
        fps: u32,
        target: VideoFrameTarget,
        clock: Arc<dyn VideoClock>,
    ) -> Result<BlackVideoFallbackHandle, Box<dyn std::error::Error>> {
        if width == 0 || height == 0 {
            return Err("black video width and height must be greater than zero".into());
        }
        if fps == 0 {
            return Err("black video fps must be greater than zero".into());
        }
        let frame_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("black video dimensions are too large")?;
        let shared = Arc::new(BlackVideoFallbackShared {
            stopping: AtomicBool::new(false),
            external_video: AtomicBool::new(false),
            wake_lock: Mutex::new(()),
            wake: Condvar::new(),
            join_handle: Mutex::new(None),
            last_error: Mutex::new(None),
        });
        let waiter: Weak<BlackVideoFallbackShared> = Arc::downgrade(&shared);
        clock.on_advance(waiter);
        let worker_shared = shared.clone();
        let frame_duration = Duration::from_secs_f64(1.0 / fps as f64);
        let join_handle = thread::spawn(move || {
            let _exit = WorkerExit(&*clock);
            black_frame_worker(&worker_shared, &*clock, frame_bytes, frame_duration, target)
        });
        *shared.join_handle.lock().unwrap() = Some(join_handle);
        Ok(BlackVideoFallbackHandle { shared })
    }
}

fn black_frame_worker(
    shared: &BlackVideoFallbackShared,
    clock: &dyn VideoClock,
    frame_bytes: usize,
    frame_duration: Duration,
    target: VideoFrameTarget,
) {
    let frame = vec![0; frame_bytes];
    let mut next_frame_at = clock.now();
    let mut guard = shared.wake_lock.lock().unwrap();

    loop {
        if shared.stopping.load(Ordering::Acquire) {
            break;
        }
        if shared.external_video.load(Ordering::Acquire) {
            // External video never hands back, so idle until stopped.
            guard = wait_on(clock, &shared.wake, guard, None);
            continue;
        }
        if clock.now() < next_frame_at {
            guard = wait_on(clock, &shared.wake, guard, Some(next_frame_at));
            continue;
        }

        drop(guard);
        if let Err(err) = target(&frame) {
            *shared.last_error.lock().unwrap() = Some(format!(
                "could not deliver black fallback video frame: {err}"
            ));
        }
        // Advance from the media deadline so frame delivery work does not
        // accumulate as permanent playback-clock drift.
        next_frame_at += frame_duration;
        guard = shared.wake_lock.lock().unwrap();
    }
}

impl BlackVideoFallbackHandle {
    pub(crate) fn external_video_started(&self) {
        self.shared.external_video.store(true, Ordering::Release);
    }

    pub(crate) fn stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        let _guard = self.shared.wake_lock.lock().unwrap();
        self.shared.wake.notify_all();
    }

    pub(crate) fn finish(&self) {
        self.stop();
        if let Some(join_handle) = self.shared.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }
    }

    pub(crate) fn last_error(&self) -> Option<String> {
        self.shared.last_error.lock().unwrap().clone()
    }
}
