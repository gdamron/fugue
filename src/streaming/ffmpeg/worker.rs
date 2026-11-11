//! The stream worker: runs ffmpeg sessions, feeds them the queued audio and
//! video, and restarts them when they end early.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::process::{finish_session, start_session, FfmpegSession};
use super::FfmpegStreamBackend;

pub(super) fn worker(shared: Arc<FfmpegStreamBackend>) {
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

fn queues_drained(shared: &FfmpegStreamBackend) -> bool {
    shared.audio_ring.is_empty() && shared.video_queue.lock().unwrap().is_empty()
}
