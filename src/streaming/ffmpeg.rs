//! Reusable native ffmpeg audio/video streaming backend.
//!
//! The backend owns process lifecycle, raw audio/video queues, command
//! construction, restart behavior, and diagnostics. Module adapters are
//! responsible for graph ports, module-specific config parsing, and any
//! user-facing controls.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
    pub gop_seconds: u32,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FfmpegCommandSpec {
    program: String,
    args: Vec<String>,
}

impl FfmpegCommandSpec {
    fn for_ports(config: &FfmpegStreamConfig, audio_port: u16, video_port: u16) -> Self {
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

        if config.constant_video_bitrate {
            args.extend([
                "-minrate".to_string(),
                config.video_bitrate.clone(),
                "-maxrate".to_string(),
                config.video_bitrate.clone(),
                "-bufsize".to_string(),
                config.video_bitrate.clone(),
            ]);
        }

        args.extend([
            "-c:a".to_string(),
            config.audio_encoder.clone(),
            "-b:a".to_string(),
            config.audio_bitrate.clone(),
        ]);

        if let Some(archive_path) = &config.tee_to_disk {
            args.extend([
                "-flags".to_string(),
                "+global_header".to_string(),
                "-map".to_string(),
                "1:v:0".to_string(),
                "-map".to_string(),
                "0:a:0".to_string(),
                "-f".to_string(),
                "tee".to_string(),
                format!(
                    "[f=flv]{}|[onfail=ignore]{}",
                    escape_tee_slave_name(&config.url),
                    escape_tee_slave_name(archive_path)
                ),
            ]);
        } else {
            args.extend(["-f".to_string(), "flv".to_string(), config.url.clone()]);
        }

        Self {
            program: config.ffmpeg_path.clone(),
            args,
        }
    }
}

fn escape_tee_slave_name(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '\'' | '|' | '[' | ']') || character.is_whitespace() {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
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
        let join_handle = thread::spawn(move || worker(worker_shared));
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
    error.replace(url, "<redacted-stream-url>")
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
    frames: Vec<Option<Box<[u8]>>>,
    capacity: usize,
    frame_bytes: usize,
    read_index: usize,
    write_index: usize,
    len: usize,
    dropped: usize,
}

impl VideoFrameQueue {
    fn new(capacity: usize, frame_bytes: usize) -> Self {
        Self {
            frames: (0..capacity).map(|_| None).collect(),
            capacity,
            frame_bytes,
            read_index: 0,
            write_index: 0,
            len: 0,
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
        if self.len == self.capacity {
            self.read_index = self.advance(self.read_index);
            self.len -= 1;
            self.dropped += 1;
        }
        match &mut self.frames[self.write_index] {
            Some(slot) => slot.copy_from_slice(frame),
            slot @ None => *slot = Some(frame.to_vec().into_boxed_slice()),
        }
        self.write_index = self.advance(self.write_index);
        self.len += 1;
        Ok(())
    }

    fn pop_into(&mut self, frame: &mut [u8]) -> bool {
        if self.len == 0 {
            return false;
        }
        frame.copy_from_slice(
            self.frames[self.read_index]
                .as_deref()
                .expect("queued video frame slot is initialized"),
        );
        self.read_index = self.advance(self.read_index);
        self.len -= 1;
        true
    }

    fn is_empty(&self) -> bool {
        self.len == 0
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

struct FfmpegSession {
    child: Child,
    audio: TcpStream,
    video: TcpStream,
    stderr_reader: Option<JoinHandle<()>>,
}

fn worker(shared: Arc<FfmpegStreamBackend>) {
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

fn start_session(shared: Arc<FfmpegStreamBackend>) -> Result<FfmpegSession, String> {
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

fn run_session(shared: &FfmpegStreamBackend, session: &mut FfmpegSession) -> SessionOutcome {
    let mut audio_bytes = Vec::with_capacity(4096 * 8);
    let mut video_frame = vec![0; shared.config.video_frame_bytes()];
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

        let has_video_frame = shared
            .video_queue
            .lock()
            .unwrap()
            .pop_into(&mut video_frame);
        if has_video_frame {
            if let Err(err) = session.video.write_all(&video_frame) {
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

fn queues_drained(shared: &FfmpegStreamBackend) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> FfmpegStreamConfig {
        FfmpegStreamConfig {
            ffmpeg_path: "ffmpeg".to_string(),
            url: "rtmp://example.test/live/fugue".to_string(),
            tee_to_disk: None,
            width: 640,
            height: 360,
            fps: 30,
            sample_rate: 48_000,
            video_encoder: "libx264".to_string(),
            audio_encoder: "aac".to_string(),
            video_bitrate: "2500k".to_string(),
            audio_bitrate: "128k".to_string(),
            gop_seconds: 2,
            constant_video_bitrate: false,
            audio_buffer_frames: DEFAULT_AUDIO_BUFFER_FRAMES,
            video_queue_frames: DEFAULT_VIDEO_QUEUE_FRAMES,
        }
    }

    #[test]
    fn builds_ffmpeg_command_for_raw_audio_video_to_flv() {
        let mut config = minimal_config();
        config.sample_rate = 44_100;
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
        assert!(!args.contains("-minrate"));
    }

    #[test]
    fn builds_constant_bitrate_arguments_when_requested() {
        let mut config = minimal_config();
        config.video_bitrate = "10000k".to_string();
        config.constant_video_bitrate = true;
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);
        let args = spec.args.join(" ");

        assert!(args.contains("-minrate 10000k -maxrate 10000k -bufsize 10000k"));
    }

    #[test]
    fn builds_tee_output_with_explicit_maps_and_escaped_slave_names() {
        let mut config = minimal_config();
        config.url = "rtmp://example.test/live/fugue|backup".to_string();
        config.tee_to_disk = Some("archive captures/session's [mix].mkv".to_string());
        let spec = FfmpegCommandSpec::for_ports(&config, 12_345, 23_456);

        let output_index = spec.args.iter().position(|arg| arg == "tee").unwrap() + 1;
        assert_eq!(
            spec.args[output_index],
            "[f=flv]rtmp://example.test/live/fugue\\|backup|[onfail=ignore]archive\\ captures/session\\'s\\ \\[mix\\].mkv"
        );
        let args = spec.args.join(" ");
        assert!(args.contains("-flags +global_header"));
        assert!(args.contains("-map 1:v:0 -map 0:a:0 -f tee"));
    }

    #[test]
    fn redacts_stream_urls_from_diagnostics() {
        let url = "rtmps://example.test/live/secret-stream-key";
        let error = redact_stream_url(format!("could not open {url}"), url);

        assert_eq!(error, "could not open <redacted-stream-url>");
        assert!(!error.contains("secret-stream-key"));
    }

    #[test]
    fn video_queue_reuses_allocated_slots_and_drops_oldest_frame() {
        let mut queue = VideoFrameQueue::new(2, 4);
        queue.push(&[1; 4]).unwrap();
        queue.push(&[2; 4]).unwrap();
        let slot_addresses: Vec<usize> = queue
            .frames
            .iter()
            .map(|frame| frame.as_deref().unwrap().as_ptr() as usize)
            .collect();
        queue.push(&[3; 4]).unwrap();

        let mut frame = [0; 4];
        assert!(queue.pop_into(&mut frame));
        assert_eq!(frame, [2; 4]);
        assert!(queue.pop_into(&mut frame));
        assert_eq!(frame, [3; 4]);
        assert!(!queue.pop_into(&mut frame));
        assert_eq!(queue.dropped, 1);
        assert_eq!(
            queue
                .frames
                .iter()
                .map(|frame| frame.as_deref().unwrap().as_ptr() as usize)
                .collect::<Vec<_>>(),
            slot_addresses
        );
    }

    #[test]
    fn missing_ffmpeg_error_is_clear() {
        let err =
            FfmpegStreamBackend::validate_ffmpeg("fugue-definitely-missing-ffmpeg").unwrap_err();
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

        fn config_with_fake(path: &PathBuf, url: &str) -> FfmpegStreamConfig {
            FfmpegStreamConfig {
                ffmpeg_path: path.to_string_lossy().into_owned(),
                url: url.to_string(),
                tee_to_disk: None,
                width: 2,
                height: 2,
                fps: 2,
                sample_rate: 48_000,
                video_encoder: "libx264".to_string(),
                audio_encoder: "aac".to_string(),
                video_bitrate: "2500k".to_string(),
                audio_bitrate: "128k".to_string(),
                gop_seconds: 2,
                constant_video_bitrate: false,
                audio_buffer_frames: 64,
                video_queue_frames: 2,
            }
        }

        fn push_smoke_media(backend: &FfmpegStreamBackend) {
            for frame_index in 0..10 {
                let mut frame = vec![0u8; 16 * 16 * 4];
                for pixel in frame.chunks_exact_mut(4) {
                    pixel[0] = (frame_index * 20) as u8;
                    pixel[1] = 64;
                    pixel[2] = 192;
                    pixel[3] = 255;
                }
                backend.push_video_rgba(&frame).unwrap();

                for sample_index in 0..4_800 {
                    let phase = (sample_index as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU;
                    let sample = phase.sin() * 0.1;
                    backend.push_audio(sample, sample);
                }
            }
        }

        #[test]
        fn streams_audio_and_video_to_fake_ffmpeg() {
            let fake = fake_ffmpeg("stream");
            FfmpegStreamBackend::validate_ffmpeg(fake.to_str().unwrap()).unwrap();
            let backend =
                FfmpegStreamBackend::start(config_with_fake(&fake, "rtmp://example.test/live"))
                    .unwrap();

            backend.push_video_rgba(&[255; 16]).unwrap();
            for _ in 0..8 {
                backend.push_audio(0.25, -0.25);
            }

            backend.finish();
            let stats = backend.stats();
            assert!(stats.audio_frames_sent > 0, "{stats:?}");
            assert_eq!(stats.video_frames_sent, 1, "{stats:?}");
            assert_eq!(stats.audio_frames_dropped, 0, "{stats:?}");
            assert_eq!(stats.video_frames_dropped, 0, "{stats:?}");
        }

        #[test]
        fn rejects_bad_video_frame_size() {
            let fake = fake_ffmpeg("bad-frame");
            let backend =
                FfmpegStreamBackend::start(config_with_fake(&fake, "rtmp://example.test/live"))
                    .unwrap();

            let err = backend.push_video_rgba(&[0; 15]).unwrap_err();
            assert!(err.contains("expected RGBA frame"));
            backend.finish();
        }

        #[test]
        fn restarts_after_unexpected_ffmpeg_exit() {
            let fake = fake_ffmpeg("exit");
            let backend =
                FfmpegStreamBackend::start(config_with_fake(&fake, "rtmp://example.test/exit"))
                    .unwrap();

            backend.push_video_rgba(&[128; 16]).unwrap();
            for _ in 0..8 {
                backend.push_audio(0.1, 0.1);
            }

            let deadline = Instant::now() + Duration::from_secs(2);
            while backend.stats().restarts == 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }

            backend.finish();
            let stats = backend.stats();
            assert!(stats.restarts > 0, "{stats:?}");
            assert!(stats.last_error.is_some(), "{stats:?}");
        }

        #[test]
        fn real_ffmpeg_writes_local_flv_when_enabled() {
            if std::env::var("FUGUE_RTMP_SINK_REAL_FFMPEG").ok().as_deref() != Some("1") {
                eprintln!("set FUGUE_RTMP_SINK_REAL_FFMPEG=1 to run real ffmpeg smoke");
                return;
            }

            FfmpegStreamBackend::validate_ffmpeg("ffmpeg").unwrap();
            let dir = temp_dir("real-ffmpeg");
            fs::create_dir_all(&dir).unwrap();
            let output = dir.join("smoke.flv");
            let backend = FfmpegStreamBackend::start(FfmpegStreamConfig {
                ffmpeg_path: "ffmpeg".to_string(),
                url: output.to_string_lossy().into_owned(),
                tee_to_disk: None,
                width: 16,
                height: 16,
                fps: 5,
                sample_rate: 48_000,
                video_encoder: "libx264".to_string(),
                audio_encoder: "aac".to_string(),
                video_bitrate: "200k".to_string(),
                audio_bitrate: "64k".to_string(),
                gop_seconds: 2,
                constant_video_bitrate: false,
                audio_buffer_frames: 512,
                video_queue_frames: 8,
            })
            .unwrap();

            push_smoke_media(&backend);

            backend.finish();
            let stats = backend.stats();
            assert!(stats.audio_frames_sent > 0, "{stats:?}");
            assert!(stats.video_frames_sent > 0, "{stats:?}");
            let metadata = fs::metadata(&output).unwrap();
            assert!(metadata.len() > 0, "expected non-empty FLV at {output:?}");
        }

        #[test]
        fn real_ffmpeg_tees_stream_to_local_archive_when_enabled() {
            if std::env::var("FUGUE_RTMP_SINK_REAL_FFMPEG").ok().as_deref() != Some("1") {
                eprintln!("set FUGUE_RTMP_SINK_REAL_FFMPEG=1 to run real ffmpeg smoke");
                return;
            }

            FfmpegStreamBackend::validate_ffmpeg("ffmpeg").unwrap();
            let dir = temp_dir("real-ffmpeg-tee");
            let archive_dir = dir.join("archive captures");
            fs::create_dir_all(&archive_dir).unwrap();
            let stream_output = dir.join("stream.flv");
            let archive_output = archive_dir.join("session's [mix].mkv");
            let backend = FfmpegStreamBackend::start(FfmpegStreamConfig {
                ffmpeg_path: "ffmpeg".to_string(),
                url: stream_output.to_string_lossy().into_owned(),
                tee_to_disk: Some(archive_output.to_string_lossy().into_owned()),
                width: 16,
                height: 16,
                fps: 5,
                sample_rate: 48_000,
                video_encoder: "libx264".to_string(),
                audio_encoder: "aac".to_string(),
                video_bitrate: "200k".to_string(),
                audio_bitrate: "64k".to_string(),
                gop_seconds: 2,
                constant_video_bitrate: false,
                audio_buffer_frames: 512,
                video_queue_frames: 8,
            })
            .unwrap();

            push_smoke_media(&backend);
            backend.finish();

            let stats = backend.stats();
            assert!(stats.audio_frames_sent > 0, "{stats:?}");
            assert!(stats.video_frames_sent > 0, "{stats:?}");
            assert_eq!(stats.last_error, None, "{stats:?}");
            assert!(fs::metadata(&stream_output).unwrap().len() > 0);
            assert!(fs::metadata(&archive_output).unwrap().len() > 0);
        }
    }
}
