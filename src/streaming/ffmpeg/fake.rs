//! A recording fake encoder for tests: no process, socket or sleep.
//!
//! Each session keeps every byte it is sent. Sessions follow a script, in
//! start order; sessions past the end of the script run normally.

use std::io;
use std::sync::{Arc, Mutex};

use super::worker::{EncoderSession, SessionOutcome, SessionStarter};
use super::FfmpegStreamBackend;

/// What one scripted session does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum FakeSession {
    /// Accepts every write and never exits on its own.
    #[default]
    Run,
    /// Fails to start with this error.
    FailToStart(String),
    /// Reports an unexpected exit, with this message, at its first check.
    ExitOnFirstCheck(String),
    /// Reports an unexpected exit, with this message, once it has accepted
    /// this many writes (audio and video together).
    ExitAfterWrites(usize, String),
    /// Accepts this many writes, then fails the next one.
    FailWriteAfter(usize),
}

/// What one session received.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FakeSessionRecord {
    /// Every audio byte, in order: f32 little-endian, left then right.
    pub audio: Vec<u8>,
    /// Every video frame, in order.
    pub video: Vec<Vec<u8>>,
    /// Whether the worker closed the session.
    pub closed: bool,
}

impl FakeSessionRecord {
    /// The audio as (left, right) frames.
    pub(crate) fn audio_frames(&self) -> Vec<(f32, f32)> {
        let (frames, rest) = self.audio.as_chunks::<8>();
        assert!(rest.is_empty(), "audio ends mid-frame");
        frames
            .iter()
            .map(|frame| {
                let (left, right) = frame.split_at(4);
                (
                    f32::from_le_bytes(left.try_into().unwrap()),
                    f32::from_le_bytes(right.try_into().unwrap()),
                )
            })
            .collect()
    }
}

#[derive(Default)]
struct FakeState {
    script: Vec<FakeSession>,
    /// Start attempts so far, failed ones included.
    starts: usize,
    /// Sessions that started, in order.
    sessions: Vec<FakeSessionRecord>,
}

/// Starts fake sessions and keeps what each one received.
#[derive(Clone, Default)]
pub(crate) struct FakeEncoder {
    state: Arc<Mutex<FakeState>>,
}

impl FakeEncoder {
    /// A fake whose sessions follow `script`, in start order.
    pub(crate) fn scripted(script: impl IntoIterator<Item = FakeSession>) -> Self {
        let fake = Self::default();
        fake.state.lock().unwrap().script = script.into_iter().collect();
        fake
    }

    /// Start attempts so far, failed ones included.
    pub(crate) fn starts(&self) -> usize {
        self.state.lock().unwrap().starts
    }

    /// What each session that started received, in start order.
    pub(crate) fn sessions(&self) -> Vec<FakeSessionRecord> {
        self.state.lock().unwrap().sessions.clone()
    }

    /// Every audio frame any session received, in order.
    pub(crate) fn audio_frames(&self) -> Vec<(f32, f32)> {
        self.sessions()
            .iter()
            .flat_map(FakeSessionRecord::audio_frames)
            .collect()
    }
}

impl SessionStarter for FakeEncoder {
    fn start(&self, _shared: &Arc<FfmpegStreamBackend>) -> Result<Box<dyn EncoderSession>, String> {
        let mut state = self.state.lock().unwrap();
        let behavior = state.script.get(state.starts).cloned().unwrap_or_default();
        state.starts += 1;
        if let FakeSession::FailToStart(error) = behavior {
            return Err(error);
        }
        state.sessions.push(FakeSessionRecord::default());
        Ok(Box::new(FakeRunningSession {
            state: self.state.clone(),
            index: state.sessions.len() - 1,
            behavior,
            writes: 0,
        }))
    }
}

struct FakeRunningSession {
    state: Arc<Mutex<FakeState>>,
    index: usize,
    behavior: FakeSession,
    writes: usize,
}

impl FakeRunningSession {
    fn write(&mut self, record: impl FnOnce(&mut FakeSessionRecord)) -> io::Result<()> {
        if let FakeSession::FailWriteAfter(limit) = self.behavior {
            if self.writes >= limit {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fake write failed",
                ));
            }
        }
        self.writes += 1;
        record(&mut self.state.lock().unwrap().sessions[self.index]);
        Ok(())
    }
}

impl EncoderSession for FakeRunningSession {
    fn check_exit(&mut self) -> Option<SessionOutcome> {
        match &self.behavior {
            FakeSession::ExitOnFirstCheck(message) => {
                Some(SessionOutcome::UnexpectedExit(message.clone()))
            }
            FakeSession::ExitAfterWrites(limit, message) if self.writes >= *limit => {
                Some(SessionOutcome::UnexpectedExit(message.clone()))
            }
            _ => None,
        }
    }

    fn write_audio(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write(|session| session.audio.extend_from_slice(bytes))
    }

    fn write_video(&mut self, frame: &[u8]) -> io::Result<()> {
        self.write(|session| session.video.push(frame.to_vec()))
    }

    fn close(self: Box<Self>) {
        self.state.lock().unwrap().sessions[self.index].closed = true;
    }
}

/// A pause hook that never waits, and stops the stream once the worker finds
/// nothing queued, so a worker run on the test thread always ends.
pub(crate) fn stop_when_idle(backend: &FfmpegStreamBackend, pause: super::Pause) {
    if pause == super::Pause::Idle {
        backend.stop();
    }
}
