//! Paced black frames for a stream with no picture of its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
    join_handle: Mutex<Option<JoinHandle<()>>>,
    last_error: Mutex<Option<String>>,
}

impl BlackVideoFallback {
    pub(crate) fn start(
        width: u32,
        height: u32,
        fps: u32,
        target: VideoFrameTarget,
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
            join_handle: Mutex::new(None),
            last_error: Mutex::new(None),
        });
        let worker_shared = shared.clone();
        let join_handle = thread::spawn(move || {
            let frame = vec![0; frame_bytes];
            let frame_duration = Duration::from_secs_f64(1.0 / fps as f64);
            let mut next_frame_at = Instant::now();

            while !worker_shared.stopping.load(Ordering::Acquire) {
                if worker_shared.external_video.load(Ordering::Acquire) {
                    thread::sleep(frame_duration);
                    continue;
                }

                let now = Instant::now();
                if now < next_frame_at {
                    thread::sleep(next_frame_at - now);
                }
                if worker_shared.stopping.load(Ordering::Acquire) {
                    break;
                }
                if !worker_shared.external_video.load(Ordering::Acquire) {
                    if let Err(err) = target(&frame) {
                        *worker_shared.last_error.lock().unwrap() = Some(format!(
                            "could not deliver black fallback video frame: {err}"
                        ));
                    }
                    // Advance from the media deadline so frame delivery work
                    // does not accumulate as permanent playback-clock drift.
                    next_frame_at += frame_duration;
                }
            }
        });
        *shared.join_handle.lock().unwrap() = Some(join_handle);
        Ok(BlackVideoFallbackHandle { shared })
    }
}

impl BlackVideoFallbackHandle {
    pub(crate) fn external_video_started(&self) {
        self.shared.external_video.store(true, Ordering::Release);
    }

    pub(crate) fn stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
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
