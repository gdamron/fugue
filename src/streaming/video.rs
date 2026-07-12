//! Picture-only video playback for native streaming sinks.
//!
//! Decoding, frame allocation, media-clock pacing, and looping all happen on
//! a dedicated worker thread. The audio graph only interacts with the sink's
//! pre-existing audio handoff path.

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) type VideoFrameTarget = Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync + 'static>;

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
                    next_frame_at = Instant::now() + frame_duration;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoPlaybackConfig {
    pub ffmpeg_path: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub autoplay: bool,
    pub loop_enabled: bool,
}

impl VideoPlaybackConfig {
    fn frame_bytes(&self) -> Result<usize, String> {
        let pixels = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or_else(|| "background video dimensions are too large".to_string())?;
        pixels
            .checked_mul(4)
            .ok_or_else(|| "background video dimensions are too large".to_string())
    }

    fn frame_duration(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoPlaybackStats {
    pub frames_emitted: usize,
    pub loops: usize,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct VideoPlaybackHandle {
    shared: Arc<VideoPlaybackShared>,
}

pub(crate) struct VideoPlayback;

impl VideoPlayback {
    pub(crate) fn start(
        config: VideoPlaybackConfig,
        target: VideoFrameTarget,
    ) -> Result<VideoPlaybackHandle, Box<dyn std::error::Error>> {
        if config.width == 0 || config.height == 0 {
            return Err("background video width and height must be greater than zero".into());
        }
        if config.fps == 0 {
            return Err("background video fps must be greater than zero".into());
        }
        config.frame_bytes()?;
        let metadata = fs::metadata(&config.path).map_err(|err| {
            format!(
                "could not open background video {}: {err}",
                config.path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "background video path is not a file: {}",
                config.path.display()
            )
            .into());
        }

        let shared = Arc::new(VideoPlaybackShared {
            control: Mutex::new(PlaybackControl {
                playing: config.autoplay,
                loop_enabled: config.loop_enabled,
                restart_generation: 0,
                stopping: false,
            }),
            wake: Condvar::new(),
            child: Mutex::new(None),
            join_handle: Mutex::new(None),
            frames_emitted: AtomicUsize::new(0),
            loops: AtomicUsize::new(0),
            last_error: Mutex::new(None),
            config,
            target,
        });

        let worker_shared = shared.clone();
        let join_handle = thread::spawn(move || playback_worker(worker_shared));
        *shared.join_handle.lock().unwrap() = Some(join_handle);

        Ok(VideoPlaybackHandle { shared })
    }
}

impl VideoPlaybackHandle {
    pub(crate) fn play(&self) {
        let mut control = self.shared.control.lock().unwrap();
        control.playing = true;
        self.shared.wake.notify_all();
    }

    pub(crate) fn pause(&self) {
        let mut control = self.shared.control.lock().unwrap();
        control.playing = false;
        self.shared.wake.notify_all();
    }

    pub(crate) fn restart(&self) {
        {
            let mut control = self.shared.control.lock().unwrap();
            control.restart_generation = control.restart_generation.wrapping_add(1);
            control.playing = true;
            self.shared.wake.notify_all();
        }
        self.shared.kill_decoder();
    }

    pub(crate) fn set_loop_enabled(&self, enabled: bool) {
        let mut control = self.shared.control.lock().unwrap();
        control.loop_enabled = enabled;
        self.shared.wake.notify_all();
    }

    pub(crate) fn stop(&self) {
        {
            let mut control = self.shared.control.lock().unwrap();
            control.stopping = true;
            self.shared.wake.notify_all();
        }
        self.shared.kill_decoder();
    }

    pub(crate) fn finish(&self) {
        self.stop();
        if let Some(join_handle) = self.shared.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }
    }

    pub(crate) fn stats(&self) -> VideoPlaybackStats {
        VideoPlaybackStats {
            frames_emitted: self.shared.frames_emitted.load(Ordering::Acquire),
            loops: self.shared.loops.load(Ordering::Acquire),
            last_error: self.shared.last_error.lock().unwrap().clone(),
        }
    }
}

struct VideoPlaybackShared {
    config: VideoPlaybackConfig,
    target: VideoFrameTarget,
    control: Mutex<PlaybackControl>,
    wake: Condvar,
    child: Mutex<Option<Child>>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
    frames_emitted: AtomicUsize,
    loops: AtomicUsize,
    last_error: Mutex<Option<String>>,
}

impl VideoPlaybackShared {
    fn kill_decoder(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    fn set_error(&self, error: impl Into<String>) {
        *self.last_error.lock().unwrap() = Some(error.into());
    }

    fn set_playing(&self, playing: bool) {
        let mut control = self.control.lock().unwrap();
        control.playing = playing;
        self.wake.notify_all();
    }
}

#[derive(Debug)]
struct PlaybackControl {
    playing: bool,
    loop_enabled: bool,
    restart_generation: u64,
    stopping: bool,
}

#[derive(Debug, Clone, Copy)]
struct ControlSnapshot {
    loop_enabled: bool,
    restart_generation: u64,
    stopping: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct FfmpegVideoCommandSpec {
    program: String,
    args: Vec<String>,
}

impl FfmpegVideoCommandSpec {
    fn new(config: &VideoPlaybackConfig) -> Self {
        let filter = format!(
            "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2:color=black,fps={}",
            config.width, config.height, config.width, config.height, config.fps
        );
        Self {
            program: config.ffmpeg_path.clone(),
            args: vec![
                "-hide_banner".to_string(),
                "-loglevel".to_string(),
                "error".to_string(),
                "-nostdin".to_string(),
                "-i".to_string(),
                config.path.to_string_lossy().into_owned(),
                "-map".to_string(),
                "0:v:0".to_string(),
                "-an".to_string(),
                "-vf".to_string(),
                filter,
                "-pix_fmt".to_string(),
                "rgba".to_string(),
                "-f".to_string(),
                "rawvideo".to_string(),
                "pipe:1".to_string(),
            ],
        }
    }
}

struct Decoder {
    stdout: ChildStdout,
}

enum DecodeOutcome {
    End,
    Restart,
    Stop,
    Failed(String),
}

enum FrameRead {
    Frame,
    End,
}

fn playback_worker(shared: Arc<VideoPlaybackShared>) {
    let mut preserve_deadline = false;
    let mut next_frame_at = Instant::now();

    loop {
        let snapshot = wait_until_playing(&shared);
        if snapshot.stopping {
            break;
        }
        if !preserve_deadline {
            next_frame_at = Instant::now();
        }

        let mut decoder = match spawn_decoder(&shared) {
            Ok(decoder) => decoder,
            Err(err) => {
                shared.set_error(err);
                shared.set_playing(false);
                preserve_deadline = false;
                continue;
            }
        };
        let current = control_snapshot(&shared);
        if current.stopping || current.restart_generation != snapshot.restart_generation {
            shared.kill_decoder();
            let _ = reap_decoder(&shared);
            preserve_deadline = false;
            if current.stopping {
                break;
            }
            continue;
        }

        let outcome = decode_frames(
            &shared,
            &mut decoder,
            snapshot.restart_generation,
            &mut next_frame_at,
        );
        let status = reap_decoder(&shared);

        match outcome {
            DecodeOutcome::Stop => break,
            DecodeOutcome::Restart => preserve_deadline = false,
            DecodeOutcome::Failed(err) => {
                shared.set_error(err);
                shared.set_playing(false);
                preserve_deadline = false;
            }
            DecodeOutcome::End => {
                let current = control_snapshot(&shared);
                if current.stopping {
                    break;
                }
                if current.restart_generation != snapshot.restart_generation {
                    preserve_deadline = false;
                    continue;
                }
                if let Some(status) = status {
                    if !status.success() {
                        shared.set_error(format!(
                            "background video decoder exited with status: {status}"
                        ));
                        shared.set_playing(false);
                        preserve_deadline = false;
                        continue;
                    }
                }
                if current.loop_enabled {
                    shared.loops.fetch_add(1, Ordering::Relaxed);
                    preserve_deadline = true;
                } else {
                    shared.set_playing(false);
                    preserve_deadline = false;
                }
            }
        }
    }

    shared.kill_decoder();
    let _ = reap_decoder(&shared);
}

fn wait_until_playing(shared: &VideoPlaybackShared) -> ControlSnapshot {
    let mut control = shared.control.lock().unwrap();
    while !control.playing && !control.stopping {
        control = shared.wake.wait(control).unwrap();
    }
    ControlSnapshot {
        loop_enabled: control.loop_enabled,
        restart_generation: control.restart_generation,
        stopping: control.stopping,
    }
}

fn control_snapshot(shared: &VideoPlaybackShared) -> ControlSnapshot {
    let control = shared.control.lock().unwrap();
    ControlSnapshot {
        loop_enabled: control.loop_enabled,
        restart_generation: control.restart_generation,
        stopping: control.stopping,
    }
}

fn spawn_decoder(shared: &VideoPlaybackShared) -> Result<Decoder, String> {
    let spec = FfmpegVideoCommandSpec::new(&shared.config);
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("could not start background video decoder: {err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "background video decoder stdout was unavailable".to_string())?;
    *shared.child.lock().unwrap() = Some(child);
    Ok(Decoder { stdout })
}

fn decode_frames(
    shared: &VideoPlaybackShared,
    decoder: &mut Decoder,
    restart_generation: u64,
    next_frame_at: &mut Instant,
) -> DecodeOutcome {
    let frame_bytes = match shared.config.frame_bytes() {
        Ok(frame_bytes) => frame_bytes,
        Err(err) => return DecodeOutcome::Failed(err),
    };
    let frame_duration = shared.config.frame_duration();
    let mut frame = vec![0; frame_bytes];

    loop {
        match read_frame(&mut decoder.stdout, &mut frame) {
            Ok(FrameRead::Frame) => {}
            Ok(FrameRead::End) => {
                return changed_outcome(shared, restart_generation, DecodeOutcome::End)
            }
            Err(err) => {
                return changed_outcome(
                    shared,
                    restart_generation,
                    DecodeOutcome::Failed(format!(
                        "background video decoder produced an incomplete frame: {err}"
                    )),
                )
            }
        }

        match wait_for_delivery(shared, restart_generation, next_frame_at) {
            DecodeOutcome::End => {}
            outcome => return outcome,
        }

        if let Err(err) = (shared.target)(&frame) {
            shared.set_error(format!("could not deliver background video frame: {err}"));
        }
        shared.frames_emitted.fetch_add(1, Ordering::Relaxed);
        *next_frame_at = Instant::now() + frame_duration;
    }
}

fn read_frame(reader: &mut impl Read, frame: &mut [u8]) -> io::Result<FrameRead> {
    let mut filled = 0;
    while filled < frame.len() {
        match reader.read(&mut frame[filled..])? {
            0 if filled == 0 => return Ok(FrameRead::End),
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("expected {} bytes, received {filled}", frame.len()),
                ))
            }
            count => filled += count,
        }
    }
    Ok(FrameRead::Frame)
}

fn wait_for_delivery(
    shared: &VideoPlaybackShared,
    restart_generation: u64,
    deadline: &mut Instant,
) -> DecodeOutcome {
    let mut control = shared.control.lock().unwrap();
    loop {
        if control.stopping {
            return DecodeOutcome::Stop;
        }
        if control.restart_generation != restart_generation {
            return DecodeOutcome::Restart;
        }
        if !control.playing {
            control = shared.wake.wait(control).unwrap();
            *deadline = Instant::now();
            continue;
        }

        let now = Instant::now();
        if now >= *deadline {
            return DecodeOutcome::End;
        }
        let (next_control, _) = shared.wake.wait_timeout(control, *deadline - now).unwrap();
        control = next_control;
    }
}

fn changed_outcome(
    shared: &VideoPlaybackShared,
    restart_generation: u64,
    unchanged: DecodeOutcome,
) -> DecodeOutcome {
    let current = control_snapshot(shared);
    if current.stopping {
        DecodeOutcome::Stop
    } else if current.restart_generation != restart_generation {
        DecodeOutcome::Restart
    } else {
        unchanged
    }
}

fn reap_decoder(shared: &VideoPlaybackShared) -> Option<ExitStatus> {
    let child = shared.child.lock().unwrap().take();
    child.and_then(|mut child| child.wait().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: PathBuf) -> VideoPlaybackConfig {
        VideoPlaybackConfig {
            ffmpeg_path: "ffmpeg".to_string(),
            path,
            width: 640,
            height: 360,
            fps: 30,
            autoplay: true,
            loop_enabled: true,
        }
    }

    #[test]
    fn builds_picture_only_rgba_decode_command() {
        let config = config(PathBuf::from("loop with spaces.mp4"));
        let spec = FfmpegVideoCommandSpec::new(&config);
        let args = spec.args.join(" ");

        assert_eq!(spec.program, "ffmpeg");
        assert!(args.contains("-i loop with spaces.mp4 -map 0:v:0 -an"));
        assert!(args.contains("scale=640:360:force_original_aspect_ratio=decrease"));
        assert!(args.contains("pad=640:360:(ow-iw)/2:(oh-ih)/2:color=black,fps=30"));
        assert!(args.ends_with("-pix_fmt rgba -f rawvideo pipe:1"));
    }

    #[test]
    fn rejects_missing_background_video_before_starting_worker() {
        let target: VideoFrameTarget = Arc::new(|_| Ok(()));
        let err = VideoPlayback::start(config(PathBuf::from("definitely-missing.mp4")), target)
            .err()
            .expect("missing file should fail");
        assert!(err.to_string().contains("could not open background video"));
    }

    #[test]
    fn black_fallback_emits_paced_frames_until_external_video_takes_over() {
        let frames = Arc::new(AtomicUsize::new(0));
        let target_frames = frames.clone();
        let target: VideoFrameTarget = Arc::new(move |frame| {
            assert_eq!(frame, &[0; 16]);
            target_frames.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let fallback = BlackVideoFallback::start(2, 2, 100, target).unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        while frames.load(Ordering::Acquire) < 3 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let before_takeover = frames.load(Ordering::Acquire);
        assert!(before_takeover >= 3);

        fallback.external_video_started();
        thread::sleep(Duration::from_millis(50));
        fallback.finish();
        assert!(frames.load(Ordering::Acquire) <= before_takeover + 1);
        assert_eq!(fallback.last_error(), None);
    }

    #[test]
    fn real_ffmpeg_decodes_local_video_when_enabled() {
        if std::env::var("FUGUE_BACKGROUND_VIDEO_REAL_FFMPEG")
            .ok()
            .as_deref()
            != Some("1")
        {
            eprintln!("set FUGUE_BACKGROUND_VIDEO_REAL_FFMPEG=1 to run real ffmpeg playback smoke");
            return;
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fugue-video-real-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let video = dir.join("loop.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=0x3060c0:s=16x16:r=5:d=0.6",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());

        let frames = Arc::new(AtomicUsize::new(0));
        let target_frames = frames.clone();
        let target: VideoFrameTarget = Arc::new(move |frame| {
            assert_eq!(frame.len(), 16 * 16 * 4);
            target_frames.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let mut config = config(video);
        config.width = 16;
        config.height = 16;
        config.fps = 5;
        config.loop_enabled = false;
        let playback = VideoPlayback::start(config, target).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while playback.stats().frames_emitted < 3 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        playback.finish();

        assert!(frames.load(Ordering::Acquire) >= 3);
        assert_eq!(playback.stats().last_error, None);
    }

    #[cfg(unix)]
    mod unix_fake_ffmpeg {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn fixture(frame_count: usize) -> (PathBuf, PathBuf) {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("fugue-video-playback-{nanos}"));
            fs::create_dir_all(&dir).unwrap();
            let ffmpeg = dir.join("ffmpeg");
            let video = dir.join("loop.mp4");
            fs::write(&video, b"fake video").unwrap();
            let script = format!(
                r#"#!/bin/sh
python3 - <<'PY'
import sys
for value in range(1, {frame_count} + 1):
    sys.stdout.buffer.write(bytes([value]) * 16)
sys.stdout.flush()
PY
"#
            );
            fs::write(&ffmpeg, script).unwrap();
            let mut permissions = fs::metadata(&ffmpeg).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&ffmpeg, permissions).unwrap();
            (ffmpeg, video)
        }

        fn fake_config(ffmpeg: PathBuf, video: PathBuf) -> VideoPlaybackConfig {
            VideoPlaybackConfig {
                ffmpeg_path: ffmpeg.to_string_lossy().into_owned(),
                path: video,
                width: 2,
                height: 2,
                fps: 20,
                autoplay: true,
                loop_enabled: false,
            }
        }

        #[test]
        fn emits_frames_on_media_clock() {
            let (ffmpeg, video) = fixture(3);
            let received = Arc::new(Mutex::new(Vec::new()));
            let target_received = received.clone();
            let target: VideoFrameTarget = Arc::new(move |frame| {
                target_received
                    .lock()
                    .unwrap()
                    .push((Instant::now(), frame.to_vec()));
                Ok(())
            });
            let playback = VideoPlayback::start(fake_config(ffmpeg, video), target).unwrap();

            let deadline = Instant::now() + Duration::from_secs(2);
            while playback.stats().frames_emitted < 3 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            playback.finish();

            let received = received.lock().unwrap();
            assert_eq!(received.len(), 3);
            assert_eq!(received[0].1, vec![1; 16]);
            assert_eq!(received[2].1, vec![3; 16]);
            assert!(received[2].0.duration_since(received[0].0) >= Duration::from_millis(80));
        }

        #[test]
        fn supports_pause_restart_and_loop_control() {
            let (ffmpeg, video) = fixture(1);
            let frames = Arc::new(AtomicUsize::new(0));
            let target_frames = frames.clone();
            let target: VideoFrameTarget = Arc::new(move |_| {
                target_frames.fetch_add(1, Ordering::Relaxed);
                Ok(())
            });
            let mut config = fake_config(ffmpeg, video);
            config.autoplay = false;
            config.loop_enabled = true;
            let playback = VideoPlayback::start(config, target).unwrap();

            thread::sleep(Duration::from_millis(75));
            assert_eq!(frames.load(Ordering::Acquire), 0);

            playback.play();
            let deadline = Instant::now() + Duration::from_secs(2);
            while frames.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(playback.stats().loops > 0);

            playback.pause();
            thread::sleep(Duration::from_millis(75));
            let paused_at = frames.load(Ordering::Acquire);
            thread::sleep(Duration::from_millis(100));
            assert_eq!(frames.load(Ordering::Acquire), paused_at);

            playback.set_loop_enabled(false);
            playback.restart();
            let deadline = Instant::now() + Duration::from_secs(2);
            while frames.load(Ordering::Acquire) == paused_at && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            playback.finish();
            assert!(frames.load(Ordering::Acquire) > paused_at);
        }
    }
}
