//! Picture-only video playback for native streaming sinks.
//!
//! Decoding, frame allocation, media-clock pacing, and looping all happen on
//! a dedicated worker thread. The audio graph only interacts with the sink's
//! pre-existing audio handoff path.

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::decoder::FfmpegVideoCommandSpec;
use super::VideoFrameTarget;

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
    pub(super) shared: Arc<VideoPlaybackShared>,
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
        let mut control = self.shared.control.lock().unwrap();
        control.restart_generation = control.restart_generation.wrapping_add(1);
        control.playing = true;
        self.shared.wake.notify_all();
        // Kill under the control lock, and only an older generation's
        // decoder: once the lock is released the worker may already have
        // started the new generation's decoder, which must keep running.
        self.shared.kill_stale_decoder(control.restart_generation);
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

pub(super) struct VideoPlaybackShared {
    config: VideoPlaybackConfig,
    target: VideoFrameTarget,
    control: Mutex<PlaybackControl>,
    wake: Condvar,
    /// The running decoder and the restart generation it was started for.
    pub(super) child: Mutex<Option<(u64, Child)>>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
    frames_emitted: AtomicUsize,
    loops: AtomicUsize,
    last_error: Mutex<Option<String>>,
}

impl VideoPlaybackShared {
    pub(super) fn kill_decoder(&self) {
        if let Some((_, child)) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    /// Kills the running decoder unless it was started for `generation`.
    pub(super) fn kill_stale_decoder(&self, generation: u64) {
        if let Some((started_for, child)) = self.child.lock().unwrap().as_mut() {
            if *started_for != generation {
                let _ = child.kill();
            }
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
        let mut decoder = match spawn_decoder(&shared, snapshot.restart_generation) {
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
            !preserve_deadline,
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

fn spawn_decoder(shared: &VideoPlaybackShared, generation: u64) -> Result<Decoder, String> {
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
    *shared.child.lock().unwrap() = Some((generation, child));
    Ok(Decoder { stdout })
}

fn decode_frames(
    shared: &VideoPlaybackShared,
    decoder: &mut Decoder,
    restart_generation: u64,
    next_frame_at: &mut Instant,
    mut reset_deadline_on_first_frame: bool,
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
        if reset_deadline_on_first_frame {
            // Decoder startup is outside the media timeline. Anchor initial
            // playback to the first decoded frame, not process spawn time.
            *next_frame_at = Instant::now();
            reset_deadline_on_first_frame = false;
        }

        match wait_for_delivery(shared, restart_generation, next_frame_at) {
            DecodeOutcome::End => {}
            outcome => return outcome,
        }

        if let Err(err) = (shared.target)(&frame) {
            shared.set_error(format!("could not deliver background video frame: {err}"));
        }
        shared.frames_emitted.fetch_add(1, Ordering::Relaxed);
        // Advance from the media deadline so decoding and delivery overhead
        // does not lower the effective frame rate over time.
        *next_frame_at += frame_duration;
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
    child.and_then(|(_, mut child)| child.wait().ok())
}
