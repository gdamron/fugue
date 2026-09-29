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
    Ok(config.sample_rate())
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
/// Supports every integer and float sample format cpal exposes.
pub struct AudioDriver {
    stream: Option<Stream>,
    sample_rate: u32,
    diagnostics: Arc<AudioDiagnostics>,
}

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
        let sample_rate = config.sample_rate();

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
        let log_missed_deadlines = std::env::var_os("FUGUE_AUDIO_DIAGNOSTICS_LOG").is_some();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                self.build_stream::<f32>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::F64 => {
                self.build_stream::<f64>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::I8 => {
                self.build_stream::<i8>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::I16 => {
                self.build_stream::<i16>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::I24 => self.build_stream::<cpal::I24>(
                &device,
                config.into(),
                render,
                log_missed_deadlines,
            )?,
            cpal::SampleFormat::I32 => {
                self.build_stream::<i32>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::I64 => {
                self.build_stream::<i64>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::U8 => {
                self.build_stream::<u8>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::U16 => {
                self.build_stream::<u16>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::U24 => self.build_stream::<cpal::U24>(
                &device,
                config.into(),
                render,
                log_missed_deadlines,
            )?,
            cpal::SampleFormat::U32 => {
                self.build_stream::<u32>(&device, config.into(), render, log_missed_deadlines)?
            }
            cpal::SampleFormat::U64 => {
                self.build_stream::<u64>(&device, config.into(), render, log_missed_deadlines)?
            }
            format => return Err(format!("Unsupported sample format: {format}").into()),
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

/// Writes one stereo frame in the device's sample format and channel layout.
///
/// Output is clamped to ±1 first. A mono device gets the average of both
/// channels; devices with more than two channels get left on even and right
/// on odd channels.
#[inline]
fn write_frame<T: cpal::Sample + cpal::FromSample<f32>>(frame: &mut [T], left: f32, right: f32) {
    let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
    match frame.len() {
        0 => {}
        1 => frame[0] = T::from_sample((left + right) * 0.5),
        _ => {
            let (left, right) = (T::from_sample(left), T::from_sample(right));
            for (index, sample) in frame.iter_mut().enumerate() {
                *sample = if index % 2 == 0 { left } else { right };
            }
        }
    }
}

impl AudioDriver {
    fn build_stream<T>(
        &self,
        device: &cpal::Device,
        config: cpal::StreamConfig,
        mut render: BlockRenderFn,
        log_missed_deadlines: bool,
    ) -> Result<Stream, Box<dyn std::error::Error>>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate;
        let diagnostics = self.diagnostics.clone();
        let error_diagnostics = self.diagnostics.clone();
        let mut left = [0.0f32; MAX_BLOCK];
        let mut right = [0.0f32; MAX_BLOCK];
        let stream = device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let started = Instant::now();
                render_block(
                    data,
                    channels,
                    &mut left,
                    &mut right,
                    &mut *render,
                    write_frame::<T>,
                );
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
            move |err: cpal::Error| {
                if err.kind() == cpal::ErrorKind::Xrun {
                    error_diagnostics.record_xrun();
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_frame_folds_mono_after_clamping() {
        let mut frame = [0.0f32; 1];
        write_frame(&mut frame, 2.0, 0.0);
        assert_eq!(frame, [0.5]);
    }

    #[test]
    fn write_frame_alternates_channels_beyond_stereo() {
        let mut frame = [0.0f32; 4];
        write_frame(&mut frame, 0.25, -0.5);
        assert_eq!(frame, [0.25, -0.5, 0.25, -0.5]);
    }

    #[test]
    fn write_frame_converts_to_integer_formats() {
        let mut signed = [0i16; 2];
        write_frame(&mut signed, 1.5, -1.5);
        assert!(signed[0] > 32_000 && signed[1] < -32_000);

        let mut unsigned = [0u16; 2];
        write_frame(&mut unsigned, 0.0, -1.0);
        assert!(unsigned[0].abs_diff(32_768) <= 1);
        assert_eq!(unsigned[1], 0);

        let mut wide = [<cpal::I24 as cpal::Sample>::EQUILIBRIUM; 1];
        write_frame(&mut wide, 1.0, 1.0);
        assert!(wide[0].inner() > 8_000_000);
    }

    #[test]
    fn render_block_splits_device_buffers_into_max_block_chunks() {
        let frames = MAX_BLOCK + 3;
        let mut data = vec![0.0f32; frames * 2];
        let mut left = [0.0f32; MAX_BLOCK];
        let mut right = [0.0f32; MAX_BLOCK];
        let mut calls = Vec::new();
        let mut render = |l: &mut [f32], r: &mut [f32]| {
            calls.push(l.len());
            l.fill(0.5);
            r.fill(-0.5);
        };
        render_block(
            &mut data,
            2,
            &mut left,
            &mut right,
            &mut render,
            write_frame::<f32>,
        );
        assert_eq!(calls, vec![MAX_BLOCK, 3]);
        assert!(data.chunks(2).all(|frame| frame == [0.5, -0.5]));
    }
}
