//! Audio output backend abstraction and default cpal-based implementation.

use crate::MAX_BLOCK;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::Arc;
use std::time::Instant;

use super::AudioDiagnostics;

/// Renders a block of `frames` planar stereo samples, where
/// `frames == left.len() == right.len()`. Called from the audio thread.
pub type BlockRenderFn = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// Renders a device buffer in `MAX_BLOCK`-frame chunks, converting each frame
/// to the device sample format and channel layout via `write`.
fn render_block<T>(
    data: &mut [T],
    channels: usize,
    left: &mut [f32; MAX_BLOCK],
    right: &mut [f32; MAX_BLOCK],
    render: &mut dyn FnMut(&mut [f32], &mut [f32]),
    write: fn(&mut [T], f32, f32),
) {
    if channels == 0 {
        return;
    }
    let frames = data.len() / channels;
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(MAX_BLOCK);
        render(&mut left[..n], &mut right[..n]);
        for k in 0..n {
            let base = (done + k) * channels;
            write(&mut data[base..base + channels], left[k], right[k]);
        }
        done += n;
    }
}

/// Returns the sample rate of the default audio output device.
///
/// This should be called before building an invention to ensure modules
/// are configured with the correct sample rate for the audio hardware.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::{default_sample_rate, Invention, InventionBuilder};
///
/// let sample_rate = default_sample_rate()?;
/// let invention = Invention::from_file("my_invention.json")?;
/// let builder = InventionBuilder::new(sample_rate);
/// let (runtime, handles) = builder.build(invention)?;
/// let running = runtime.start()?;
/// ```
///
/// # Errors
///
/// Returns an error if no audio output device is available.
pub fn default_sample_rate() -> Result<u32, Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let config = device.default_output_config()?;
    Ok(config.sample_rate().0)
}

/// Trait for audio output backends.
///
/// This abstraction allows different audio backends (cpal, file writer, network streamer, etc.)
/// to be used interchangeably with the invention runtime.
///
/// # Example
///
/// ```rust,ignore
/// use fugue::AudioBackend;
///
/// // Use the default AudioDriver
/// let mut audio = AudioDriver::new()?;
/// audio.start(Box::new(|| {
///     // Return next sample
///     0.0
/// }))?;
/// ```
pub trait AudioBackend: Send {
    /// Returns the sample rate of the audio backend in Hz.
    fn sample_rate(&self) -> u32;

    /// Starts audio output with the given block render function.
    ///
    /// `render` is called from the audio thread to fill planar stereo output:
    /// `render(left, right)` with `left.len() == right.len()`. Backends may call
    /// it with any block length up to [`MAX_BLOCK`].
    fn start(&mut self, render: BlockRenderFn) -> Result<(), Box<dyn std::error::Error>>;

    /// Stops audio output.
    fn stop(&mut self);

    /// Returns live callback diagnostics when the backend can collect them.
    fn diagnostics(&self) -> Option<Arc<AudioDiagnostics>> {
        None
    }
}

/// Default audio backend using the cpal library.
///
/// Sends audio to the system's default output device.
/// Supports F32, I16, and U16 sample formats.
pub struct AudioDriver {
    stream: Option<Stream>,
    sample_rate: u32,
    diagnostics: Arc<AudioDiagnostics>,
}

/// Safety: AudioDriver is safe to send between threads. The contained cpal::Stream
/// uses `PhantomData<*mut ()>` which prevents auto-impl of Send, but the stream's
/// audio callback runs on its own dedicated thread regardless of which thread
/// owns the Stream handle. We only call play() and drop() on it.
unsafe impl Send for AudioDriver {}

impl AudioDriver {
    /// Creates a new AudioDriver using the system's default output device.
    ///
    /// Returns an error if no output device is available.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;

        Ok(Self {
            stream: None,
            sample_rate,
            diagnostics: Arc::new(AudioDiagnostics::new()),
        })
    }
}

impl AudioBackend for AudioDriver {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self, render: BlockRenderFn) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate().0;
        let diagnostics = self.diagnostics.clone();
        let log_missed_deadlines = std::env::var_os("FUGUE_AUDIO_DIAGNOSTICS_LOG").is_some();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mut render = render;
                let mut left = [0.0f32; MAX_BLOCK];
                let mut right = [0.0f32; MAX_BLOCK];
                Self::build_stream::<f32>(
                    &device,
                    &config.into(),
                    channels,
                    sample_rate,
                    diagnostics.clone(),
                    log_missed_deadlines,
                    move |data: &mut [f32]| {
                        render_block(
                            data,
                            channels,
                            &mut left,
                            &mut right,
                            &mut *render,
                            write_frame_f32,
                        );
                    },
                )?
            }
            cpal::SampleFormat::I16 => {
                let mut render = render;
                let mut left = [0.0f32; MAX_BLOCK];
                let mut right = [0.0f32; MAX_BLOCK];
                Self::build_stream::<i16>(
                    &device,
                    &config.into(),
                    channels,
                    sample_rate,
                    diagnostics.clone(),
                    log_missed_deadlines,
                    move |data: &mut [i16]| {
                        render_block(
                            data,
                            channels,
                            &mut left,
                            &mut right,
                            &mut *render,
                            write_frame_i16,
                        );
                    },
                )?
            }
            cpal::SampleFormat::U16 => {
                let mut render = render;
                let mut left = [0.0f32; MAX_BLOCK];
                let mut right = [0.0f32; MAX_BLOCK];
                Self::build_stream::<u16>(
                    &device,
                    &config.into(),
                    channels,
                    sample_rate,
                    diagnostics.clone(),
                    log_missed_deadlines,
                    move |data: &mut [u16]| {
                        render_block(
                            data,
                            channels,
                            &mut left,
                            &mut right,
                            &mut *render,
                            write_frame_u16,
                        );
                    },
                )?
            }
            _ => return Err("Unsupported sample format".into()),
        };

        stream.play()?;
        self.stream = Some(stream);

        Ok(())
    }

    fn stop(&mut self) {
        self.stream = None;
    }

    fn diagnostics(&self) -> Option<Arc<AudioDiagnostics>> {
        Some(self.diagnostics.clone())
    }
}

fn write_channels<T: Copy>(frame: &mut [T], left: T, right: T) {
    match frame.len() {
        0 => {}
        1 => frame[0] = left,
        _ => {
            for (index, sample) in frame.iter_mut().enumerate() {
                *sample = if index % 2 == 0 { left } else { right };
            }
        }
    }
}

fn write_frame_f32(frame: &mut [f32], left: f32, right: f32) {
    let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
    if frame.len() == 1 {
        frame[0] = (left + right) * 0.5;
        return;
    }
    write_channels(frame, left, right);
}

fn write_frame_i16(frame: &mut [i16], left: f32, right: f32) {
    let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
    if frame.len() == 1 {
        frame[0] = (((left + right) * 0.5) * i16::MAX as f32) as i16;
        return;
    }
    write_channels(
        frame,
        (left * i16::MAX as f32) as i16,
        (right * i16::MAX as f32) as i16,
    );
}

fn write_frame_u16(frame: &mut [u16], left: f32, right: f32) {
    let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
    if frame.len() == 1 {
        frame[0] = ((((left + right) * 0.5) + 1.0) * 0.5 * u16::MAX as f32) as u16;
        return;
    }
    write_channels(
        frame,
        ((left + 1.0) * 0.5 * u16::MAX as f32) as u16,
        ((right + 1.0) * 0.5 * u16::MAX as f32) as u16,
    );
}

impl AudioDriver {
    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        channels: usize,
        sample_rate: u32,
        diagnostics: Arc<AudioDiagnostics>,
        log_missed_deadlines: bool,
        mut callback: impl FnMut(&mut [T]) + Send + 'static,
    ) -> Result<Stream, Box<dyn std::error::Error>>
    where
        T: cpal::Sample + cpal::SizedSample,
    {
        let error_diagnostics = diagnostics.clone();
        let stream = device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let started = Instant::now();
                callback(data);
                let callback_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
                let buffer_period_ns = buffer_period_ns(data.len(), channels, sample_rate);
                if diagnostics.record_callback(callback_ns, buffer_period_ns)
                    && log_missed_deadlines
                {
                    eprintln!(
                        "Audio callback missed deadline: {:.3} ms > {:.3} ms",
                        callback_ns as f64 / 1_000_000.0,
                        buffer_period_ns as f64 / 1_000_000.0
                    );
                }
            },
            move |err| {
                error_diagnostics.record_xrun();
                eprintln!("Stream error: {}", err);
            },
            None,
        )?;

        Ok(stream)
    }
}

#[inline]
fn buffer_period_ns(sample_count: usize, channels: usize, sample_rate: u32) -> u64 {
    if channels == 0 || sample_rate == 0 {
        return 0;
    }
    let frames = sample_count / channels;
    ((frames as u128 * 1_000_000_000u128) / u128::from(sample_rate)) as u64
}
