//! Native ffmpeg backend for [`RtmpSink`](super::RtmpSink).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{RtmpSink, RtmpSinkHandle, RtmpSinkStats};

const DEFAULT_BUFFER_FRAMES: usize = 65_536;
const DEFAULT_VIDEO_QUEUE_FRAMES: usize = 8;
const STDERR_TAIL_BYTES: usize = 16 * 1024;

pub(super) type SharedHandle = Arc<NativeRtmpSinkShared>;

pub(super) fn build(
    config: &serde_json::Value,
    sample_rate: u32,
) -> Result<(RtmpSink, RtmpSinkHandle), Box<dyn std::error::Error>> {
    let config = RtmpSinkConfig::from_json(config, sample_rate)?;
    NativeRtmpSinkShared::validate_ffmpeg(&config.ffmpeg_path)?;
    RtmpSink::new_native(config)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpSinkConfig {
    pub ffmpeg_path: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub video_encoder: String,
    pub audio_encoder: String,
    pub video_bitrate: String,
    pub audio_bitrate: String,
    pub gop_seconds: u32,
    pub buffer_frames: usize,
    pub video_queue_frames: usize,
    pub monitor: bool,
    pub soft_clip: bool,
}

impl RtmpSinkConfig {
    fn from_json(
        config: &serde_json::Value,
        sample_rate: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let url = required_string(config, "url")?;
        let width = required_u32(config, "width")?;
        let height = required_u32(config, "height")?;
        if width == 0 || height == 0 {
            return Err("rtmp_sink width and height must be greater than zero".into());
        }

        let fps = optional_u32(config, "fps", 30)?;
        if fps == 0 {
            return Err("rtmp_sink fps must be greater than zero".into());
        }

        let gop_seconds = optional_u32(config, "gop_seconds", 2)?;
        if gop_seconds == 0 {
            return Err("rtmp_sink gop_seconds must be greater than zero".into());
        }

        let buffer_frames = optional_usize(config, "buffer_frames", DEFAULT_BUFFER_FRAMES)?;
        if buffer_frames == 0 {
            return Err("rtmp_sink buffer_frames must be greater than zero".into());
        }

        let video_queue_frames =
            optional_usize(config, "video_queue_frames", DEFAULT_VIDEO_QUEUE_FRAMES)?;
        if video_queue_frames == 0 {
            return Err("rtmp_sink video_queue_frames must be greater than zero".into());
        }

        Ok(Self {
            ffmpeg_path: optional_string(config, "ffmpeg_path", "ffmpeg"),
            url,
            width,
            height,
            fps,
            sample_rate,
            video_encoder: optional_string(config, "video_encoder", "libx264"),
            audio_encoder: optional_string(config, "audio_encoder", "aac"),
            video_bitrate: optional_string(config, "video_bitrate", "2500k"),
            audio_bitrate: optional_string(config, "audio_bitrate", "128k"),
            gop_seconds,
            buffer_frames,
            video_queue_frames,
            monitor: optional_bool(config, "monitor", false),
            soft_clip: optional_bool(config, "soft_clip", true),
        })
    }

    fn video_frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FfmpegCommandSpec {
    program: String,
    args: Vec<String>,
}

impl FfmpegCommandSpec {
    fn for_ports(config: &RtmpSinkConfig, audio_port: u16, video_port: u16) -> Self {
        let gop_frames = config.fps.saturating_mul(config.gop_seconds).max(1);
        let mut args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "warning".to_string(),
            "-nostdin".to_string(),
            "-f".to_string(),
            "f32le".to_string(),
            "-ar".to_string(),
            config.sample_rate.to_string(),
            "-ac".to_string(),
            "2".to_string(),
            "-i".to_string(),
            format!("tcp://127.0.0.1:{audio_port}"),
            "-f".to_string(),
            "rawvideo".to_string(),
            "-pix_fmt".to_string(),
            "rgba".to_string(),
            "-s".to_string(),
            format!("{}x{}", config.width, config.height),
            "-framerate".to_string(),
            config.fps.to_string(),
            "-i".to_string(),
            format!("tcp://127.0.0.1:{video_port}"),
            "-c:v".to_string(),
            config.video_encoder.clone(),
            "-b:v".to_string(),
            config.video_bitrate.clone(),
            "-g".to_string(),
            gop_frames.to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
        ];

        if config.video_encoder == "libx264" {
            args.extend(["-preset".to_string(), "veryfast".to_string()]);
        }

        args.extend([
            "-c:a".to_string(),
            config.audio_encoder.clone(),
            "-b:a".to_string(),
            config.audio_bitrate.clone(),
            "-f".to_string(),
            "flv".to_string(),
            config.url.clone(),
        ]);

        Self {
            program: config.ffmpeg_path.clone(),
            args,
        }
    }
}

impl RtmpSink {
    pub fn new_native(
        config: RtmpSinkConfig,
    ) -> Result<(Self, RtmpSinkHandle), Box<dyn std::error::Error>> {
        let soft_clip = config.soft_clip;
        let monitor = config.monitor;
        let shared = NativeRtmpSinkShared::start(config)?;
        Ok(Self::from_shared(shared, soft_clip, monitor))
    }
}

pub(super) struct NativeRtmpSinkShared {
    config: RtmpSinkConfig,
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

impl NativeRtmpSinkShared {
    fn start(config: RtmpSinkConfig) -> Result<SharedHandle, Box<dyn std::error::Error>> {
        let shared = Arc::new(Self {
            audio_ring: AudioFrameRing::new(config.buffer_frames),
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
        let join_handle = thread::spawn(move || worker(worker_shared));
        *shared.join_handle.lock().unwrap() = Some(join_handle);

        Ok(shared)
    }

    fn validate_ffmpeg(path: &str) -> Result<(), Box<dyn std::error::Error>> {
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
    pub(super) fn push_audio(&self, left: f32, right: f32) {
        self.audio_ring.push(left, right);
    }

    #[inline]
    pub(super) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub(super) fn push_video_rgba(&self, frame: &[u8]) -> Result<(), String> {
        let mut queue = self.video_queue.lock().unwrap();
        queue.push(frame)
    }

    pub(super) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    pub(super) fn finish(&self) {
        self.stop();
        if let Some(join_handle) = self.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }
    }

    pub(super) fn stats(&self) -> RtmpSinkStats {
        RtmpSinkStats {
            audio_frames_sent: self.audio_frames_sent.load(Ordering::Acquire),
            audio_frames_dropped: self.audio_ring.dropped.load(Ordering::Acquire),
            video_frames_sent: self.video_frames_sent.load(Ordering::Acquire),
            video_frames_dropped: self.video_queue.lock().unwrap().dropped,
            restarts: self.restarts.load(Ordering::Acquire),
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }

    fn set_error(&self, error: impl Into<String>) {
        *self.last_error.lock().unwrap() = Some(error.into());
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

struct AudioFrameRing {
    slots: Box<[AtomicU64]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
    capacity: usize,
    dropped: AtomicUsize,
}

impl AudioFrameRing {
    fn new(capacity: usize) -> Self {
        let slots = (0..capacity)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
            capacity,
            dropped: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn push(&self, left: f32, right: f32) {
        let write = self.write_index.load(Ordering::Relaxed);
        let next = self.advance(write);
        if next == self.read_index.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        self.slots[write].store(pack_frame(left, right), Ordering::Relaxed);
        self.write_index.store(next, Ordering::Release);
    }

    fn pop(&self) -> Option<(f32, f32)> {
        let read = self.read_index.load(Ordering::Relaxed);
        if read == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        let frame = unpack_frame(self.slots[read].load(Ordering::Relaxed));
        self.read_index.store(self.advance(read), Ordering::Release);
        Some(frame)
    }

    fn is_empty(&self) -> bool {
        self.read_index.load(Ordering::Acquire) == self.write_index.load(Ordering::Acquire)
    }

    #[inline]
    fn advance(&self, index: usize) -> usize {
        let next = index + 1;
        if next == self.capacity {
            0
        } else {
            next
        }
    }
}

struct VideoFrameQueue {
    frames: VecDeque<Vec<u8>>,
    capacity: usize,
    frame_bytes: usize,
    dropped: usize,
}

impl VideoFrameQueue {
    fn new(capacity: usize, frame_bytes: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(capacity),
            capacity,
            frame_bytes,
            dropped: 0,
        }
    }

    fn push(&mut self, frame: &[u8]) -> Result<(), String> {
        if frame.len() != self.frame_bytes {
            return Err(format!(
                "rtmp_sink expected RGBA frame with {} bytes, got {}",
                self.frame_bytes,
                frame.len()
            ));
        }
        if self.frames.len() == self.capacity {
            self.frames.pop_front();
            self.dropped += 1;
        }
        self.frames.push_back(frame.to_vec());
        Ok(())
    }

    fn pop(&mut self) -> Option<Vec<u8>> {
        self.frames.pop_front()
    }

    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

struct FfmpegSession {
    child: Child,
    audio: TcpStream,
    video: TcpStream,
    stderr_reader: Option<JoinHandle<()>>,
}

fn worker(shared: Arc<NativeRtmpSinkShared>) {
    while !shared.stopping.load(Ordering::Acquire) || !queues_drained(&shared) {
        match start_session(shared.clone()) {
            Ok(mut session) => {
                let outcome = run_session(&shared, &mut session);
                finish_session(session);

                match outcome {
                    SessionOutcome::GracefulStop => break,
                    SessionOutcome::UnexpectedExit(message) | SessionOutcome::IoError(message) => {
                        if shared.stopping.load(Ordering::Acquire) {
                            shared.set_error(message);
                            break;
                        }
                        shared.restarts.fetch_add(1, Ordering::Relaxed);
                        shared.set_error(message);
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            Err(err) => {
                shared.set_error(err);
                if shared.stopping.load(Ordering::Acquire) {
                    break;
                }
                shared.restarts.fetch_add(1, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

#[derive(Debug)]
enum SessionOutcome {
    GracefulStop,
    UnexpectedExit(String),
    IoError(String),
}

fn start_session(shared: Arc<NativeRtmpSinkShared>) -> Result<FfmpegSession, String> {
    let audio_listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("audio bind failed: {err}"))?;
    let video_listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|err| format!("video bind failed: {err}"))?;
    audio_listener
        .set_nonblocking(true)
        .map_err(|err| format!("audio listener nonblocking failed: {err}"))?;
    video_listener
        .set_nonblocking(true)
        .map_err(|err| format!("video listener nonblocking failed: {err}"))?;

    let audio_port = audio_listener
        .local_addr()
        .map_err(|err| err.to_string())?
        .port();
    let video_port = video_listener
        .local_addr()
        .map_err(|err| err.to_string())?
        .port();
    let spec = FfmpegCommandSpec::for_ports(&shared.config, audio_port, video_port);

    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn ffmpeg '{}': {err}", spec.program))?;

    let stderr_reader = child.stderr.take().map(|mut stderr| {
        let reader_shared = shared.clone();
        thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            loop {
                match stderr.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => reader_shared.append_stderr(&buffer[..n]),
                    Err(_) => break,
                }
            }
        })
    });

    let (audio, video) = accept_input_streams(&mut child, &audio_listener, &video_listener)?;
    audio
        .set_nodelay(true)
        .map_err(|err| format!("audio stream nodelay failed: {err}"))?;
    video
        .set_nodelay(true)
        .map_err(|err| format!("video stream nodelay failed: {err}"))?;

    Ok(FfmpegSession {
        child,
        audio,
        video,
        stderr_reader,
    })
}

fn accept_input_streams(
    child: &mut Child,
    audio_listener: &TcpListener,
    video_listener: &TcpListener,
) -> Result<(TcpStream, TcpStream), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut audio = None;
    let mut video = None;

    while audio.is_none() || video.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Err("ffmpeg did not connect raw audio/video inputs in time".to_string());
        }

        if audio.is_none() {
            match audio_listener.accept() {
                Ok((stream, _)) => audio = Some(stream),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("audio accept failed: {err}")),
            }
        }
        if video.is_none() {
            match video_listener.accept() {
                Ok((stream, _)) => video = Some(stream),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("video accept failed: {err}")),
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("ffmpeg exited before connecting inputs: {status}"));
            }
            Ok(None) => {}
            Err(err) => return Err(format!("ffmpeg status check failed: {err}")),
        }

        thread::sleep(Duration::from_millis(5));
    }

    Ok((audio.unwrap(), video.unwrap()))
}

fn run_session(shared: &NativeRtmpSinkShared, session: &mut FfmpegSession) -> SessionOutcome {
    let mut audio_bytes = Vec::with_capacity(4096 * 8);
    loop {
        if shared.stopping.load(Ordering::Acquire) && queues_drained(shared) {
            return SessionOutcome::GracefulStop;
        }

        match session.child.try_wait() {
            Ok(Some(status)) => {
                let mut message = format!("ffmpeg exited unexpectedly: {status}");
                if let Some(stderr) = shared.stderr_tail_string() {
                    message.push_str("; stderr: ");
                    message.push_str(stderr.trim());
                }
                return SessionOutcome::UnexpectedExit(message);
            }
            Ok(None) => {}
            Err(err) => {
                return SessionOutcome::IoError(format!("ffmpeg status check failed: {err}"));
            }
        }

        let mut did_work = false;
        audio_bytes.clear();
        while audio_bytes.len() < audio_bytes.capacity() {
            let Some((left, right)) = shared.audio_ring.pop() else {
                break;
            };
            audio_bytes.extend_from_slice(&left.to_le_bytes());
            audio_bytes.extend_from_slice(&right.to_le_bytes());
        }
        if !audio_bytes.is_empty() {
            if let Err(err) = session.audio.write_all(&audio_bytes) {
                return SessionOutcome::IoError(format!("ffmpeg audio write failed: {err}"));
            }
            shared
                .audio_frames_sent
                .fetch_add(audio_bytes.len() / 8, Ordering::Relaxed);
            did_work = true;
        }

        let frame = shared.video_queue.lock().unwrap().pop();
        if let Some(frame) = frame {
            if let Err(err) = session.video.write_all(&frame) {
                return SessionOutcome::IoError(format!("ffmpeg video write failed: {err}"));
            }
            shared.video_frames_sent.fetch_add(1, Ordering::Relaxed);
            did_work = true;
        }

        if !did_work {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn finish_session(mut session: FfmpegSession) {
    drop(session.audio);
    drop(session.video);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match session.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = session.child.kill();
                let _ = session.child.wait();
                break;
            }
            Err(_) => break,
        }
    }
    if let Some(reader) = session.stderr_reader.take() {
        let _ = reader.join();
    }
}

fn queues_drained(shared: &NativeRtmpSinkShared) -> bool {
    shared.audio_ring.is_empty() && shared.video_queue.lock().unwrap().is_empty()
}

#[inline]
fn pack_frame(left: f32, right: f32) -> u64 {
    ((left.to_bits() as u64) << 32) | right.to_bits() as u64
}

#[inline]
fn unpack_frame(frame: u64) -> (f32, f32) {
    (
        f32::from_bits((frame >> 32) as u32),
        f32::from_bits(frame as u32),
    )
}

fn required_string(
    config: &serde_json::Value,
    key: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("rtmp_sink requires config.{key}").into())
}

fn optional_string(config: &serde_json::Value, key: &str, default: &str) -> String {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or(default)
        .to_string()
}

fn required_u32(config: &serde_json::Value, key: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let value = config
        .get(key)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("rtmp_sink requires numeric config.{key}"))?;
    u32::try_from(value).map_err(|_| format!("rtmp_sink config.{key} is too large").into())
}

fn optional_u32(
    config: &serde_json::Value,
    key: &str,
    default: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    match config.get(key).and_then(|value| value.as_u64()) {
        Some(value) => {
            u32::try_from(value).map_err(|_| format!("rtmp_sink config.{key} is too large").into())
        }
        None => Ok(default),
    }
}

fn optional_usize(
    config: &serde_json::Value,
    key: &str,
    default: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    match config.get(key).and_then(|value| value.as_u64()) {
        Some(value) => usize::try_from(value)
            .map_err(|_| format!("rtmp_sink config.{key} is too large").into()),
        None => Ok(default),
    }
}

fn optional_bool(config: &serde_json::Value, key: &str, default: bool) -> bool {
    config
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Module, SinkModule};
    use serde_json::json;

    fn minimal_config() -> serde_json::Value {
        json!({
            "url": "rtmp://example.test/live/fugue",
            "width": 640,
            "height": 360
        })
    }

    #[test]
    fn parses_defaults() {
        let config = RtmpSinkConfig::from_json(&minimal_config(), 48_000).unwrap();

        assert_eq!(config.ffmpeg_path, "ffmpeg");
        assert_eq!(config.url, "rtmp://example.test/live/fugue");
        assert_eq!(config.width, 640);
        assert_eq!(config.height, 360);
        assert_eq!(config.fps, 30);
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.video_encoder, "libx264");
        assert_eq!(config.audio_encoder, "aac");
        assert_eq!(config.video_bitrate, "2500k");
        assert_eq!(config.audio_bitrate, "128k");
        assert_eq!(config.gop_seconds, 2);
        assert_eq!(config.buffer_frames, DEFAULT_BUFFER_FRAMES);
        assert_eq!(config.video_queue_frames, DEFAULT_VIDEO_QUEUE_FRAMES);
        assert!(!config.monitor);
        assert!(config.soft_clip);
    }

    #[test]
    fn rejects_invalid_required_values() {
        let missing_url = json!({ "width": 640, "height": 360 });
        assert!(RtmpSinkConfig::from_json(&missing_url, 48_000)
            .unwrap_err()
            .to_string()
            .contains("config.url"));

        let zero_width = json!({ "url": "rtmp://x", "width": 0, "height": 360 });
        assert!(RtmpSinkConfig::from_json(&zero_width, 48_000)
            .unwrap_err()
            .to_string()
            .contains("width and height"));

        let zero_fps = json!({ "url": "rtmp://x", "width": 640, "height": 360, "fps": 0 });
        assert!(RtmpSinkConfig::from_json(&zero_fps, 48_000)
            .unwrap_err()
            .to_string()
            .contains("fps"));
    }

    #[test]
    fn builds_ffmpeg_command_for_raw_audio_video_to_flv() {
        let mut config = RtmpSinkConfig::from_json(&minimal_config(), 44_100).unwrap();
        config.video_bitrate = "3000k".to_string();
        config.audio_bitrate = "160k".to_string();
        config.gop_seconds = 3;
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);

        assert_eq!(spec.program, "ffmpeg");
        let args = spec.args.join(" ");
        assert!(args.contains("-f f32le -ar 44100 -ac 2 -i tcp://127.0.0.1:12345"));
        assert!(args.contains(
            "-f rawvideo -pix_fmt rgba -s 640x360 -framerate 30 -i tcp://127.0.0.1:23456"
        ));
        assert!(args.contains("-c:v libx264 -b:v 3000k -g 90 -pix_fmt yuv420p"));
        assert!(args.contains("-c:a aac -b:a 160k -f flv rtmp://example.test/live/fugue"));
    }

    #[test]
    fn missing_ffmpeg_error_is_clear() {
        let err =
            NativeRtmpSinkShared::validate_ffmpeg("fugue-definitely-missing-ffmpeg").unwrap_err();
        assert!(err.to_string().contains("requires ffmpeg in PATH"));
    }

    #[cfg(unix)]
    mod unix_fake_ffmpeg {
        use super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn temp_dir(name: &str) -> PathBuf {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            std::env::temp_dir().join(format!("fugue-rtmp-sink-{name}-{nanos}"))
        }

        fn fake_ffmpeg(mode: &str) -> PathBuf {
            let dir = temp_dir(mode);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("ffmpeg");
            let script = format!(
                r#"#!/bin/sh
if [ "$1" = "-version" ]; then
  exit 0
fi
FUGUE_FAKE_FFMPEG_MODE="{mode}" python3 - "$@" <<'PY'
import os
import re
import select
import socket
import sys
import time

urls = [arg for arg in sys.argv[1:] if arg.startswith("tcp://")]
if len(urls) < 2:
    sys.exit(10)

def port(url):
    match = re.search(r":(\d+)(?:\?|$)", url)
    if not match:
        sys.exit(11)
    return int(match.group(1))

sockets = []
for url in urls[:2]:
    s = socket.create_connection(("127.0.0.1", port(url)), timeout=5)
    s.setblocking(False)
    sockets.append(s)

if os.environ.get("FUGUE_FAKE_FFMPEG_MODE") == "exit":
    sys.exit(2)

deadline = time.time() + 5
while sockets and time.time() < deadline:
    readable, _, _ = select.select(sockets, [], [], 0.05)
    for s in readable:
        try:
            data = s.recv(65536)
        except BlockingIOError:
            continue
        if not data:
            sockets.remove(s)
            s.close()

sys.exit(0 if not sockets else 12)
PY
"#
            );
            fs::write(&path, script).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
            path
        }

        fn config_with_fake(path: &PathBuf, url: &str) -> RtmpSinkConfig {
            RtmpSinkConfig::from_json(
                &json!({
                    "ffmpeg_path": path.to_string_lossy(),
                    "url": url,
                    "width": 2,
                    "height": 2,
                    "fps": 2,
                    "buffer_frames": 64,
                    "video_queue_frames": 2,
                    "monitor": true,
                    "soft_clip": false
                }),
                48_000,
            )
            .unwrap()
        }

        #[test]
        fn streams_audio_and_video_to_fake_ffmpeg() {
            let fake = fake_ffmpeg("stream");
            NativeRtmpSinkShared::validate_ffmpeg(fake.to_str().unwrap()).unwrap();
            let (mut sink, handle) =
                RtmpSink::new_native(config_with_fake(&fake, "rtmp://example.test/live")).unwrap();

            handle.push_video_rgba(&[255; 16]).unwrap();
            sink.set_input("audio_left", 0.25).unwrap();
            sink.set_input("audio_right", -0.25).unwrap();
            sink.process(8);

            let (left, right) = sink.sink_block();
            assert_eq!(left[0], 0.25);
            assert_eq!(right[0], -0.25);

            let stats = handle.finish();
            assert!(stats.audio_frames_sent > 0, "{stats:?}");
            assert_eq!(stats.video_frames_sent, 1, "{stats:?}");
            assert_eq!(stats.audio_frames_dropped, 0, "{stats:?}");
            assert_eq!(stats.video_frames_dropped, 0, "{stats:?}");
        }

        #[test]
        fn rejects_bad_video_frame_size() {
            let fake = fake_ffmpeg("bad-frame");
            let (_sink, handle) =
                RtmpSink::new_native(config_with_fake(&fake, "rtmp://example.test/live")).unwrap();

            let err = handle.push_video_rgba(&[0; 15]).unwrap_err();
            assert!(err.contains("expected RGBA frame"));
            handle.finish();
        }

        #[test]
        fn restarts_after_unexpected_ffmpeg_exit() {
            let fake = fake_ffmpeg("exit");
            let (mut sink, handle) =
                RtmpSink::new_native(config_with_fake(&fake, "rtmp://example.test/exit")).unwrap();

            handle.push_video_rgba(&[128; 16]).unwrap();
            sink.set_input("audio", 0.1).unwrap();
            sink.process(8);

            let deadline = Instant::now() + Duration::from_secs(2);
            while handle.stats().restarts == 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }

            let stats = handle.finish();
            assert!(stats.restarts > 0, "{stats:?}");
            assert!(stats.last_error.is_some(), "{stats:?}");
        }
    }
}
