//! Test stand-ins for the video decoder and clock: no processes, no waiting
//! on real time.
//!
//! A test drives a video worker by stepping a [`SteppedClock`].
//! [`SteppedClock::advance`] returns once the worker has caught up and is
//! idle again, so whatever it was going to deliver by then has been
//! delivered, and the test can assert on recorded frames straight away.

use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use super::clock::{ClockWaiter, VideoClock};
use super::decoder::{DecoderLauncher, DecoderProcess, LaunchedDecoder};
use super::{VideoPlaybackConfig, VideoRuntime};

/// A clock whose time only moves when a test advances it.
///
/// Workers wait on it with no timeout; advancing wakes them. It tracks
/// when a worker is idle, which is what lets [`advance`](Self::advance)
/// wait for the worker. Drive one worker per clock.
pub(crate) struct SteppedClock {
    origin: Instant,
    state: Mutex<SteppedState>,
    idle: Condvar,
}

struct SteppedState {
    elapsed: Duration,
    /// Bumped by every advance.
    epoch: u64,
    /// The latest epoch at which a worker went idle on the clock.
    idle_epoch: u64,
    /// Workers blocked reading a held-open fake decoder, idle until killed.
    blocked_readers: usize,
    /// The worker ended, normally or by panic, and will never go idle.
    worker_exited: bool,
    waiters: Vec<Weak<dyn ClockWaiter>>,
}

impl SteppedClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            origin: Instant::now(),
            state: Mutex::new(SteppedState {
                elapsed: Duration::ZERO,
                epoch: 0,
                idle_epoch: 0,
                blocked_readers: 0,
                worker_exited: false,
                waiters: Vec::new(),
            }),
            idle: Condvar::new(),
        })
    }

    /// Time since the clock was made.
    pub(crate) fn elapsed(&self) -> Duration {
        self.state.lock().unwrap().elapsed
    }

    /// Moves time forward by `by`, wakes the worker, and returns once it is
    /// idle again: waiting on the clock, or blocked on a held-open fake
    /// decoder (which time does not affect).
    ///
    /// Panics if the worker has ended (stopped, or panicked), so a broken
    /// worker fails its test rather than hanging it.
    pub(crate) fn advance(&self, by: Duration) {
        let (epoch, waiters) = {
            let mut state = self.state.lock().unwrap();
            state.elapsed += by;
            state.epoch += 1;
            state.waiters.retain(|waiter| waiter.strong_count() > 0);
            let waiters: Vec<_> = state.waiters.iter().filter_map(Weak::upgrade).collect();
            (state.epoch, waiters)
        };
        for waiter in waiters {
            waiter.clock_advanced();
        }
        let mut state = self.state.lock().unwrap();
        while state.idle_epoch < epoch && state.blocked_readers == 0 {
            if state.worker_exited {
                drop(state);
                panic!("the video worker exited while the test was advancing its clock");
            }
            state = self.idle.wait(state).unwrap();
        }
    }

    /// Lets the worker catch up with everything done before this call (a
    /// start, a pause, a restart) without moving time.
    pub(crate) fn settle(&self) {
        self.advance(Duration::ZERO);
    }

    fn reader_blocked(&self, blocked: bool) {
        let mut state = self.state.lock().unwrap();
        if blocked {
            state.blocked_readers += 1;
            self.idle.notify_all();
        } else {
            state.blocked_readers -= 1;
        }
    }

    /// A runtime that decodes with `decoders` and paces by this clock.
    pub(crate) fn runtime(self: &Arc<Self>, decoders: &Arc<FakeDecoders>) -> VideoRuntime {
        VideoRuntime {
            decoders: decoders.clone(),
            clock: self.clone(),
        }
    }
}

impl VideoClock for SteppedClock {
    fn now(&self) -> Instant {
        self.origin + self.state.lock().unwrap().elapsed
    }

    fn wait_limit(&self, deadline: Option<Instant>) -> Option<Duration> {
        let mut state = self.state.lock().unwrap();
        if deadline.is_some_and(|deadline| deadline <= self.origin + state.elapsed) {
            // Time moved since the worker read it: recheck instead of waiting.
            return Some(Duration::ZERO);
        }
        state.idle_epoch = state.epoch;
        self.idle.notify_all();
        None
    }

    fn on_advance(&self, waiter: Weak<dyn ClockWaiter>) {
        self.state.lock().unwrap().waiters.push(waiter);
    }

    fn worker_exited(&self) {
        // The worker may be unwinding: never panic again here.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.worker_exited = true;
        self.idle.notify_all();
    }
}

/// What one fake decode pass does.
#[derive(Debug, Clone)]
pub(crate) struct FakeRun {
    start_error: Option<String>,
    output: Vec<u8>,
    exit: Result<(), String>,
    hold_open: bool,
}

impl FakeRun {
    /// `count` frames of `frame_bytes` each; frame `n` (from 1) is all `n`.
    pub(crate) fn frames(count: u8, frame_bytes: usize) -> Self {
        Self::bytes(
            (1..=count)
                .flat_map(|value| vec![value; frame_bytes])
                .collect(),
        )
    }

    /// Raw decoder output, which may end partway through a frame.
    pub(crate) fn bytes(output: Vec<u8>) -> Self {
        Self {
            start_error: None,
            output,
            exit: Ok(()),
            hold_open: false,
        }
    }

    /// A decoder that cannot start.
    pub(crate) fn fail_to_start(error: &str) -> Self {
        Self {
            start_error: Some(error.to_string()),
            ..Self::bytes(Vec::new())
        }
    }

    /// Exits with a failed `status` once its output is read.
    pub(crate) fn exit_failure(mut self, status: &str) -> Self {
        self.exit = Err(status.to_string());
        self
    }

    /// Keeps the output open after the last byte until killed, as a decoder
    /// still working on a long video would.
    pub(crate) fn hold_open(mut self) -> Self {
        self.hold_open = true;
        self
    }
}

/// Decoders that play scripted runs from memory: launch `n` plays run `n`,
/// or the last run once they are used up.
pub(crate) struct FakeDecoders {
    clock: Arc<SteppedClock>,
    runs: Vec<FakeRun>,
    launches: Mutex<Vec<Arc<FakeOutput>>>,
    start_attempts: AtomicUsize,
}

impl FakeDecoders {
    pub(crate) fn new(clock: &Arc<SteppedClock>, runs: Vec<FakeRun>) -> Arc<Self> {
        assert!(!runs.is_empty(), "a fake decoder needs at least one run");
        Arc::new(Self {
            clock: clock.clone(),
            runs,
            launches: Mutex::new(Vec::new()),
            start_attempts: AtomicUsize::new(0),
        })
    }

    /// How many decoders were asked to start, including failed starts.
    pub(crate) fn start_attempts(&self) -> usize {
        self.start_attempts.load(Ordering::Acquire)
    }

    /// Whether the decoder from successful launch `index` was killed.
    pub(crate) fn killed(&self, index: usize) -> bool {
        self.launches.lock().unwrap()[index]
            .state
            .lock()
            .unwrap()
            .killed
    }
}

impl DecoderLauncher for FakeDecoders {
    fn launch(&self, _config: &VideoPlaybackConfig) -> Result<LaunchedDecoder, String> {
        let attempt = self.start_attempts.fetch_add(1, Ordering::AcqRel);
        let run = &self.runs[attempt.min(self.runs.len() - 1)];
        if let Some(error) = &run.start_error {
            return Err(error.clone());
        }
        let output = Arc::new(FakeOutput {
            clock: self.clock.clone(),
            state: Mutex::new(OutputState {
                bytes: run.output.clone(),
                read: 0,
                hold_open: run.hold_open,
                blocked: false,
                killed: false,
            }),
            wake: Condvar::new(),
        });
        self.launches.lock().unwrap().push(output.clone());
        Ok(LaunchedDecoder {
            frames: Box::new(FakeReader(output.clone())),
            process: Box::new(FakeProcess {
                output,
                exit: run.exit.clone(),
            }),
        })
    }
}

struct FakeOutput {
    clock: Arc<SteppedClock>,
    state: Mutex<OutputState>,
    wake: Condvar,
}

struct OutputState {
    bytes: Vec<u8>,
    read: usize,
    hold_open: bool,
    /// A reader is waiting for output that only a kill ends.
    blocked: bool,
    killed: bool,
}

struct FakeReader(Arc<FakeOutput>);

impl Read for FakeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if state.killed {
                return Ok(0);
            }
            if state.read < state.bytes.len() {
                let count = buf.len().min(state.bytes.len() - state.read);
                buf[..count].copy_from_slice(&state.bytes[state.read..state.read + count]);
                state.read += count;
                return Ok(count);
            }
            if !state.hold_open {
                return Ok(0);
            }
            if !state.blocked {
                state.blocked = true;
                self.0.clock.reader_blocked(true);
            }
            state = self.0.wake.wait(state).unwrap();
        }
    }
}

struct FakeProcess {
    output: Arc<FakeOutput>,
    exit: Result<(), String>,
}

impl DecoderProcess for FakeProcess {
    fn kill(&mut self) {
        let mut state = self.output.state.lock().unwrap();
        state.killed = true;
        // Clear the block here, not in the reader, so an advance right
        // after the kill waits for the worker to react.
        if state.blocked {
            state.blocked = false;
            self.output.clock.reader_blocked(false);
        }
        self.output.wake.notify_all();
    }

    fn wait(&mut self) -> Option<Result<(), String>> {
        if self.output.state.lock().unwrap().killed {
            return Some(Err("signal: 9 (SIGKILL)".to_string()));
        }
        Some(self.exit.clone())
    }
}
