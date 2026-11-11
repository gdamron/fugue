//! Reusable native ffmpeg audio/video streaming backend.
//!
//! The backend owns the raw audio/video queues, the worker that feeds encoder
//! sessions and restarts them, and diagnostics. Module adapters are
//! responsible for graph ports, module-specific config parsing, and any
//! user-facing controls.
//!
//! Files: `queues` holds the audio ring and video queue, `worker` the session
//! loop and the encoder session seam, `process` the ffmpeg process adapter,
//! and `command` its command line.

use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

mod command;
#[cfg(test)]
pub(crate) mod fake;
mod process;
mod queues;
#[cfg(test)]
mod tests;
mod worker;

use queues::{AudioFrameRing, VideoFrameQueue};
#[cfg(test)]
pub(crate) use worker::{Pause, SessionStarter};

pub(crate) const DEFAULT_AUDIO_BUFFER_FRAMES: usize = 65_536;
pub(crate) const DEFAULT_VIDEO_QUEUE_FRAMES: usize = 8;
const STDERR_TAIL_BYTES: usize = 16 * 1024;

pub(crate) type FfmpegStreamHandle = Arc<FfmpegStreamBackend>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FfmpegStreamConfig {
    pub ffmpeg_path: String,
    pub url: String,
    pub tee_to_disk: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub video_encoder: String,
    pub audio_encoder: String,
    pub video_bitrate: String,
    pub audio_bitrate: String,
    pub gop_duration: u32,
    pub constant_video_bitrate: bool,
    pub audio_buffer_frames: usize,
    pub video_queue_frames: usize,
}

impl FfmpegStreamConfig {
    fn video_frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FfmpegStreamStats {
    pub audio_frames_sent: usize,
    pub audio_frames_dropped: usize,
    pub video_frames_sent: usize,
    pub video_frames_dropped: usize,
    pub restarts: usize,
    pub last_error: Option<String>,
}

pub(crate) struct FfmpegStreamBackend {
    config: FfmpegStreamConfig,
    audio_ring: AudioFrameRing,
    video_queue: Mutex<VideoFrameQueue>,
    stopping: AtomicBool,
    audio_frames_sent: AtomicUsize,
    video_frames_sent: AtomicUsize,
    restarts: AtomicUsize,
    last_error: Mutex<Option<String>>,
    stderr_tail: Mutex<VecDeque<u8>>,
    worker: Mutex<Option<WorkerSlot>>,
}

/// Where the worker runs.
enum WorkerSlot {
    /// On its own thread, joined by `finish()`.
    Thread(JoinHandle<()>),
    /// On the thread that calls `finish()`, for tests.
    #[cfg(test)]
    OnFinish(Box<dyn FnOnce() + Send>),
}

impl FfmpegStreamBackend {
    pub(crate) fn start(
        config: FfmpegStreamConfig,
    ) -> Result<FfmpegStreamHandle, Box<dyn std::error::Error>> {
        let shared = Self::new(config);
        let worker_shared = shared.clone();
        let join_handle = thread::spawn(move || {
            worker::worker(&worker_shared, &process::ProcessStarter, &mut |_, pause| {
                thread::sleep(pause.duration())
            })
        });
        *shared.worker.lock().unwrap() = Some(WorkerSlot::Thread(join_handle));

        Ok(shared)
    }

    /// A stream whose worker runs on the thread that calls `finish()`, over
    /// sessions from `starter`, waiting through `pause`.
    #[cfg(test)]
    pub(crate) fn start_on_finish(
        config: FfmpegStreamConfig,
        starter: Arc<dyn SessionStarter>,
        mut pause: impl FnMut(&FfmpegStreamBackend, Pause) + Send + 'static,
    ) -> FfmpegStreamHandle {
        let shared = Self::new(config);
        let weak = Arc::downgrade(&shared);
        let run = move || {
            if let Some(shared) = weak.upgrade() {
                worker::worker(&shared, starter.as_ref(), &mut pause);
            }
        };
        *shared.worker.lock().unwrap() = Some(WorkerSlot::OnFinish(Box::new(run)));
        shared
    }

    fn new(config: FfmpegStreamConfig) -> FfmpegStreamHandle {
        Arc::new(Self {
            audio_ring: AudioFrameRing::new(config.audio_buffer_frames),
            video_queue: Mutex::new(VideoFrameQueue::new(
                config.video_queue_frames,
                config.video_frame_bytes(),
            )),
            config,
            stopping: AtomicBool::new(false),
            audio_frames_sent: AtomicUsize::new(0),
            video_frames_sent: AtomicUsize::new(0),
            restarts: AtomicUsize::new(0),
            last_error: Mutex::new(None),
            stderr_tail: Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)),
            worker: Mutex::new(None),
        })
    }

    pub(crate) fn validate_ffmpeg(path: &str) -> Result<(), Box<dyn std::error::Error>> {
        match Command::new(path)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("ffmpeg probe failed with status: {status}").into()),
            Err(err) => Err(format!("rtmp_sink requires ffmpeg in PATH: {err}").into()),
        }
    }

    #[inline]
    pub(crate) fn push_audio(&self, left: f32, right: f32) {
        self.audio_ring.push(left, right);
    }

    #[inline]
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub(crate) fn push_video_rgba(&self, frame: &[u8]) -> Result<(), String> {
        let mut queue = self.video_queue.lock().unwrap();
        queue.push(frame)
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    pub(crate) fn finish(&self) {
        self.stop();
        let worker = self.worker.lock().unwrap().take();
        match worker {
            Some(WorkerSlot::Thread(join_handle)) => {
                let _ = join_handle.join();
            }
            #[cfg(test)]
            Some(WorkerSlot::OnFinish(run)) => run(),
            None => {}
        }
    }

    /// Runs a `start_on_finish` worker now, without stopping first.
    #[cfg(test)]
    pub(crate) fn run_worker(&self) {
        let worker = self.worker.lock().unwrap().take();
        if let Some(WorkerSlot::OnFinish(run)) = worker {
            run();
        }
    }

    /// Whether no audio frame is waiting to be sent.
    #[cfg(test)]
    pub(crate) fn audio_ring_is_empty(&self) -> bool {
        self.audio_ring.is_empty()
    }

    pub(crate) fn stats(&self) -> FfmpegStreamStats {
        FfmpegStreamStats {
            audio_frames_sent: self.audio_frames_sent.load(Ordering::Acquire),
            audio_frames_dropped: self.audio_ring.dropped.load(Ordering::Acquire),
            video_frames_sent: self.video_frames_sent.load(Ordering::Acquire),
            video_frames_dropped: self.video_queue.lock().unwrap().dropped,
            restarts: self.restarts.load(Ordering::Acquire),
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }

    fn set_error(&self, error: impl Into<String>) {
        let error = redact_stream_url(error.into(), &self.config.url);
        eprintln!("ffmpeg stream error: {error}");
        *self.last_error.lock().unwrap() = Some(error);
    }

    fn append_stderr(&self, bytes: &[u8]) {
        let mut tail = self.stderr_tail.lock().unwrap();
        for byte in bytes {
            if tail.len() == STDERR_TAIL_BYTES {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }

    fn stderr_tail_string(&self) -> Option<String> {
        let mut tail = self.stderr_tail.lock().unwrap();
        if tail.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(tail.make_contiguous()).to_string())
        }
    }
}

fn redact_stream_url(error: String, url: &str) -> String {
    let redacted = error.replace(url, "<redacted-stream-url>");
    let Some((_, stream_key)) = url.rsplit_once('/') else {
        return redacted;
    };
    if stream_key.is_empty() {
        redacted
    } else {
        redacted.replace(stream_key, "<redacted-stream-key>")
    }
}
