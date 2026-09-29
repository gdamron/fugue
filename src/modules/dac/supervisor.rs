//! Keeps the cpal output stream alive.
//!
//! A dedicated control thread owns the stream. It builds the first stream when
//! playback starts. When the host reports that the stream has stopped (the
//! output device went away, or the stream was invalidated), the error callback
//! only raises a flag and wakes this thread, which then rebuilds the stream on
//! the current default output. While no device can be opened it retries with
//! backoff. Rebuilding never happens on the audio thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, SupportedStreamConfig, SupportedStreamConfigRange};

use super::driver::{buffer_period_ns, render_block, stream_error_kind, write_frame};
use super::{AudioDiagnostics, BlockRenderFn};
use crate::MAX_BLOCK;

/// First wait before retrying a stream that could not be opened.
const FIRST_RETRY: Duration = Duration::from_millis(100);

/// Longest wait between retries.
const MAX_RETRY: Duration = Duration::from_secs(2);

/// State shared by the supervisor thread and the stream callbacks.
struct Shared {
    /// The graph's render function. Only the live stream's data callback uses
    /// it, and a replaced stream is dropped before its successor is built, so
    /// the `try_lock` in the callback never contends and never blocks.
    render: Mutex<BlockRenderFn>,
    diagnostics: Arc<AudioDiagnostics>,
    /// Sample rate the graph was built at; every stream is opened at it.
    sample_rate: u32,
    log_missed_deadlines: bool,
    rebuild_requested: AtomicBool,
    shutdown: AtomicBool,
    supervisor: OnceLock<Thread>,
}

impl Shared {
    fn wake(&self) {
        if let Some(thread) = self.supervisor.get() {
            thread.unpark();
        }
    }

    /// Asks the supervisor to rebuild the stream. Safe to call from any
    /// thread, including the host's error callback.
    fn request_rebuild(&self) {
        self.rebuild_requested.store(true, Ordering::Release);
        self.wake();
    }
}

/// Owns the thread that keeps the output stream alive. Dropping it stops the
/// stream and joins the thread.
pub(super) struct Supervisor {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl Supervisor {
    /// Opens the first stream on the default output and starts supervising
    /// it. Returns an error, and leaves nothing running, if that first stream
    /// cannot be opened.
    pub(super) fn start(
        render: BlockRenderFn,
        diagnostics: Arc<AudioDiagnostics>,
        sample_rate: u32,
        log_missed_deadlines: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let shared = Arc::new(Shared {
            render: Mutex::new(render),
            diagnostics,
            sample_rate,
            log_missed_deadlines,
            rebuild_requested: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            supervisor: OnceLock::new(),
        });
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_shared = shared.clone();
        let handle = thread::Builder::new()
            .name("fugue-audio-output".into())
            .spawn(move || supervise(thread_shared, ready_tx, open_stream))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                handle: Some(handle),
            }),
            Ok(Err(message)) => {
                let _ = handle.join();
                Err(message.into())
            }
            Err(_) => {
                let _ = handle.join();
                Err("audio output thread exited before opening a stream".into())
            }
        }
    }

    /// Stops the stream and waits for the supervisor thread to exit.
    pub(super) fn stop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.wake();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The supervisor thread: opens the first stream, then rebuilds it whenever
/// a callback asks, until shutdown. `open` opens a stream; it is a parameter
/// so tests can stand in for cpal.
///
/// Wakes are unpark tokens, and calls into cpal (opening or dropping a
/// stream) may park this thread internally and consume one, e.g. while
/// waiting on a channel. So after any such call the loop re-reads the
/// shutdown and rebuild flags before it sleeps; it only parks with no cpal
/// call between that check and the park, where a token cannot be lost.
fn supervise<S>(
    shared: Arc<Shared>,
    ready: SyncSender<Result<(), String>>,
    mut open: impl FnMut(&Arc<Shared>) -> Result<S, Box<dyn std::error::Error>>,
) {
    let _ = shared.supervisor.set(thread::current());
    let mut stream = match open(&shared) {
        Ok(stream) => {
            let _ = ready.send(Ok(()));
            Some(stream)
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    drop(ready);

    let mut backoff = Backoff::default();
    let mut retry_delay = None;
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        let rebuild = shared.rebuild_requested.swap(false, Ordering::AcqRel);
        if rebuild || stream.is_none() {
            // Drop the stopped stream first, so its callbacks are gone before
            // the next stream takes over the render function.
            stream = None;
            match open(&shared) {
                Ok(next) => {
                    stream = Some(next);
                    backoff.reset();
                    retry_delay = None;
                    shared.diagnostics.record_stream_restart();
                    eprintln!("Audio output restored");
                }
                Err(error) => {
                    if backoff.is_first_failure() {
                        eprintln!("Audio output unavailable, retrying: {error}");
                    }
                    retry_delay = Some(backoff.failed());
                }
            }
            // Re-check the flags before sleeping; see above.
            if shared.shutdown.load(Ordering::Acquire)
                || shared.rebuild_requested.load(Ordering::Acquire)
            {
                continue;
            }
        }
        match retry_delay.take() {
            Some(delay) => thread::park_timeout(delay),
            None => thread::park(),
        }
    }
    drop(stream);
}

/// Retry delays for a stream that cannot be opened: doubling from
/// [`FIRST_RETRY`] up to [`MAX_RETRY`].
#[derive(Debug, Default)]
struct Backoff {
    failures: u32,
}

impl Backoff {
    fn is_first_failure(&self) -> bool {
        self.failures == 0
    }

    /// Records a failure and returns how long to wait before the next try.
    fn failed(&mut self) -> Duration {
        let delay = FIRST_RETRY
            .saturating_mul(1u32 << self.failures.min(16))
            .min(MAX_RETRY);
        self.failures = self.failures.saturating_add(1);
        delay
    }

    fn reset(&mut self) {
        self.failures = 0;
    }
}

/// Opens and starts a stream on the current default output at the graph's
/// sample rate.
fn open_stream(shared: &Arc<Shared>) -> Result<Stream, Box<dyn std::error::Error>> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No output device available")?;
    let config = output_config(&device, shared.sample_rate)?;
    let stream = build_for_format(&device, config, shared)?;
    stream.play()?;
    Ok(stream)
}

/// The device's default config when it already runs at `sample_rate`,
/// otherwise the closest supported config at that rate.
fn output_config(
    device: &cpal::Device,
    sample_rate: u32,
) -> Result<SupportedStreamConfig, Box<dyn std::error::Error>> {
    let default = device.default_output_config()?;
    if default.sample_rate() == sample_rate {
        return Ok(default);
    }
    let ranges = device.supported_output_configs()?;
    config_at_rate(&default, ranges, sample_rate).ok_or_else(|| {
        format!("the output device cannot play at the invention's {sample_rate} Hz").into()
    })
}

/// Picks a supported config at `sample_rate`, preferring the default's sample
/// format and channel count, then its channel count, then anything.
fn config_at_rate(
    default: &SupportedStreamConfig,
    ranges: impl IntoIterator<Item = SupportedStreamConfigRange>,
    sample_rate: u32,
) -> Option<SupportedStreamConfig> {
    let candidates: Vec<_> = ranges
        .into_iter()
        .filter(|range| range.contains_rate(sample_rate))
        .collect();
    let same_format = |range: &&SupportedStreamConfigRange| {
        range.sample_format() == default.sample_format() && range.channels() == default.channels()
    };
    let same_channels =
        |range: &&SupportedStreamConfigRange| range.channels() == default.channels();
    candidates
        .iter()
        .find(same_format)
        .or_else(|| candidates.iter().find(same_channels))
        .or_else(|| candidates.first())
        .cloned()?
        .try_with_sample_rate(sample_rate)
}

/// Builds a stream in the config's sample format.
fn build_for_format(
    device: &cpal::Device,
    config: SupportedStreamConfig,
    shared: &Arc<Shared>,
) -> Result<Stream, Box<dyn std::error::Error>> {
    let format = config.sample_format();
    let config = config.config();
    match format {
        SampleFormat::F32 => build_stream::<f32>(device, config, shared),
        SampleFormat::F64 => build_stream::<f64>(device, config, shared),
        SampleFormat::I8 => build_stream::<i8>(device, config, shared),
        SampleFormat::I16 => build_stream::<i16>(device, config, shared),
        SampleFormat::I24 => build_stream::<cpal::I24>(device, config, shared),
        SampleFormat::I32 => build_stream::<i32>(device, config, shared),
        SampleFormat::I64 => build_stream::<i64>(device, config, shared),
        SampleFormat::U8 => build_stream::<u8>(device, config, shared),
        SampleFormat::U16 => build_stream::<u16>(device, config, shared),
        SampleFormat::U24 => build_stream::<cpal::U24>(device, config, shared),
        SampleFormat::U32 => build_stream::<u32>(device, config, shared),
        SampleFormat::U64 => build_stream::<u64>(device, config, shared),
        format => Err(format!("Unsupported sample format: {format}").into()),
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: &Arc<Shared>,
) -> Result<Stream, Box<dyn std::error::Error>>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate;
    let data_shared = shared.clone();
    let error_shared = shared.clone();
    let mut left = [0.0f32; MAX_BLOCK];
    let mut right = [0.0f32; MAX_BLOCK];
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let started = Instant::now();
            let diagnostics = &data_shared.diagnostics;
            diagnostics.record_callback_at(started);
            match data_shared.render.try_lock() {
                Ok(mut render) => render_block(
                    data,
                    channels,
                    &mut left,
                    &mut right,
                    &mut **render,
                    write_frame::<T>,
                ),
                // Never wait on the audio thread; a render function that is
                // busy (or panicked) yields a silent buffer instead.
                Err(_) => data.fill(<T as cpal::Sample>::EQUILIBRIUM),
            }
            let callback_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            let buffer_period_ns = buffer_period_ns(data.len(), channels, sample_rate);
            if diagnostics.record_callback(callback_ns, buffer_period_ns)
                && data_shared.log_missed_deadlines
            {
                eprintln!(
                    "Audio callback missed deadline: {:.3} ms > {:.3} ms",
                    callback_ns as f64 / 1_000_000.0,
                    buffer_period_ns as f64 / 1_000_000.0
                );
            }
        },
        move |err: cpal::Error| {
            match stream_error_kind(err.kind()) {
                None => error_shared.diagnostics.record_xrun(),
                Some(kind) => {
                    error_shared.diagnostics.record_stream_error(kind);
                    if kind.stops_stream() {
                        error_shared.request_rebuild();
                    }
                }
            }
            eprintln!("Stream error: {}", err);
        },
        None,
    )?;

    Ok(stream)
}

#[cfg(test)]
mod tests;
