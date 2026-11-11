//! Reusable native ffmpeg audio/video streaming backend.
//!
//! The backend owns process lifecycle, raw audio/video queues, command
//! construction, restart behavior, and diagnostics. Module adapters are
//! responsible for graph ports, module-specific config parsing, and any
//! user-facing controls.

//! Files: `queues` holds the audio ring and video queue, `worker` the session
//! loop, `process` the ffmpeg process session, and `command` its command
//! line.

use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

mod command;
mod process;
mod queues;
#[cfg(test)]
mod tests;
mod worker;

use queues::{AudioFrameRing, VideoFrameQueue};

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
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl FfmpegStreamBackend {
    pub(crate) fn start(
        config: FfmpegStreamConfig,
    ) -> Result<FfmpegStreamHandle, Box<dyn std::error::Error>> {
        let shared = Arc::new(Self {
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
            join_handle: Mutex::new(None),
        });

        let worker_shared = shared.clone();
        let join_handle = thread::spawn(move || worker::worker(worker_shared));
        *shared.join_handle.lock().unwrap() = Some(join_handle);

        Ok(shared)
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
        if let Some(join_handle) = self.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }
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
