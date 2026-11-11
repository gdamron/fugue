//! The stream worker: runs encoder sessions, feeds them the queued audio and
//! video, and restarts them when they end early.
//!
//! An encoder session sits behind [`EncoderSession`], started by a
//! [`SessionStarter`]. Production starts an ffmpeg process fed over local TCP
//! (`super::process`); tests use a recording fake, so the whole worker can run
//! on the test thread with no process, socket or sleep.

use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::FfmpegStreamBackend;

/// One running encoder session.
pub(crate) trait EncoderSession: Send {
    /// How the session ended, if it has, without waiting.
    fn check_exit(&mut self) -> Option<SessionOutcome>;
    /// Writes interleaved f32 little-endian stereo frames.
    fn write_audio(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Writes one RGBA video frame.
    fn write_video(&mut self, frame: &[u8]) -> io::Result<()>;
    /// Closes the inputs and waits for the encoder to finish.
    fn close(self: Box<Self>);
}

/// Starts encoder sessions for a stream.
pub(crate) trait SessionStarter: Send + Sync {
    fn start(&self, shared: &Arc<FfmpegStreamBackend>) -> Result<Box<dyn EncoderSession>, String>;
}

/// Why the worker is about to wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pause {
    /// Nothing was queued on this pass.
    Idle,
    /// A session ended early; the next one starts after the wait.
    AfterExit,
    /// A session failed to start; the next try comes after the wait.
    AfterStartFailure,
}

impl Pause {
    /// How long production sleeps for this pause.
    pub(crate) fn duration(self) -> Duration {
        match self {
            Pause::Idle => Duration::from_millis(1),
            Pause::AfterExit => Duration::from_millis(100),
            Pause::AfterStartFailure => Duration::from_millis(250),
        }
    }
}

/// Waits for a [`Pause`]: production sleeps, tests return at once.
pub(crate) type PauseHook<'a> = &'a mut dyn FnMut(&FfmpegStreamBackend, Pause);

#[derive(Debug)]
pub(crate) enum SessionOutcome {
    GracefulStop,
    UnexpectedExit(String),
    IoError(String),
}

pub(super) fn worker(
    shared: &Arc<FfmpegStreamBackend>,
    starter: &dyn SessionStarter,
    pause: PauseHook<'_>,
) {
    while !shared.stopping.load(Ordering::Acquire) || !queues_drained(shared) {
        match starter.start(shared) {
            Ok(mut session) => {
                let outcome = run_session(shared, session.as_mut(), pause);
                session.close();

                match outcome {
                    SessionOutcome::GracefulStop => break,
                    SessionOutcome::UnexpectedExit(message) | SessionOutcome::IoError(message) => {
                        if shared.stopping.load(Ordering::Acquire) {
                            shared.set_error(message);
                            break;
                        }
                        shared.restarts.fetch_add(1, Ordering::Relaxed);
                        shared.set_error(message);
                        pause(shared, Pause::AfterExit);
                    }
                }
            }
            Err(err) => {
                shared.set_error(err);
                if shared.stopping.load(Ordering::Acquire) {
                    break;
                }
                shared.restarts.fetch_add(1, Ordering::Relaxed);
                pause(shared, Pause::AfterStartFailure);
            }
        }
    }
}

fn run_session(
    shared: &FfmpegStreamBackend,
    session: &mut dyn EncoderSession,
    pause: PauseHook<'_>,
) -> SessionOutcome {
    let mut audio_bytes = Vec::with_capacity(4096 * 8);
    let mut video_frame = vec![0; shared.config.video_frame_bytes()];
    loop {
        if shared.stopping.load(Ordering::Acquire) && queues_drained(shared) {
            return SessionOutcome::GracefulStop;
        }

        if let Some(outcome) = session.check_exit() {
            return outcome;
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
            if let Err(err) = session.write_audio(&audio_bytes) {
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
            if let Err(err) = session.write_video(&video_frame) {
                return SessionOutcome::IoError(format!("ffmpeg video write failed: {err}"));
            }
            shared.video_frames_sent.fetch_add(1, Ordering::Relaxed);
            did_work = true;
        }

        if !did_work {
            pause(shared, Pause::Idle);
        }
    }
}

fn queues_drained(shared: &FfmpegStreamBackend) -> bool {
    shared.audio_ring.is_empty() && shared.video_queue.lock().unwrap().is_empty()
}
