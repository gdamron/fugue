//! Audio output backend abstraction and default cpal-based implementation.

use crate::MAX_BLOCK;
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::Arc;

use super::diagnostics::StreamErrorKind;
use super::supervisor::Supervisor;
use super::AudioDiagnostics;

/// Renders a block of `frames` planar stereo samples, where
/// `frames == left.len() == right.len()`. Called from the audio thread.
pub type BlockRenderFn = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// Renders a device buffer in `MAX_BLOCK`-frame chunks, converting each frame
/// to the device sample format and channel layout via `write`.
pub(super) fn render_block<T>(
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
///
/// If the host stops the stream, e.g. because the output device went away,
/// the driver rebuilds it on the current default output at the same sample
/// rate, retrying until a device is available.
pub struct AudioDriver {
    output: Option<Supervisor>,
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
            output: None,
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
        self.stop();
        let log_missed_deadlines = std::env::var_os("FUGUE_AUDIO_DIAGNOSTICS_LOG").is_some();
        self.output = Some(Supervisor::start(
            render,
            self.diagnostics.clone(),
            self.sample_rate,
            log_missed_deadlines,
        )?);
        Ok(())
    }

    fn stop(&mut self) {
        self.output = None;
    }

    fn diagnostics(&self) -> Option<Arc<AudioDiagnostics>> {
        Some(self.diagnostics.clone())
    }
}

/// Largest `f32` below 1.0. cpal's f32 → I24/U24 conversions assume samples in
/// `-1.0..1.0` and wrap +1.0 around to negative full scale, so the positive
/// clamp stops just short of it. Other formats saturate either way.
const MAX_BELOW_ONE: f32 = 1.0 - f32::EPSILON / 2.0;

/// Writes one stereo frame in the device's sample format and channel layout.
///
/// Output is clamped to `-1.0..1.0` first. A mono device gets the average of both
/// channels; devices with more than two channels get left on even and right
/// on odd channels.
#[inline]
pub(super) fn write_frame<T: cpal::Sample + cpal::FromSample<f32>>(
    frame: &mut [T],
    left: f32,
    right: f32,
) {
    let (left, right) = (
        left.clamp(-1.0, MAX_BELOW_ONE),
        right.clamp(-1.0, MAX_BELOW_ONE),
    );
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

/// Maps a cpal error to a diagnostics kind, or `None` for an xrun.
pub(super) fn stream_error_kind(kind: cpal::ErrorKind) -> Option<StreamErrorKind> {
    Some(match kind {
        cpal::ErrorKind::Xrun => return None,
        cpal::ErrorKind::DeviceNotAvailable => StreamErrorKind::DeviceNotAvailable,
        cpal::ErrorKind::DeviceChanged => StreamErrorKind::DeviceChanged,
        cpal::ErrorKind::StreamInvalidated => StreamErrorKind::StreamInvalidated,
        cpal::ErrorKind::RealtimeDenied => StreamErrorKind::RealtimeDenied,
        _ => StreamErrorKind::Other,
    })
}

#[inline]
pub(super) fn buffer_period_ns(sample_count: usize, channels: usize, sample_rate: u32) -> u64 {
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
        assert!((frame[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn write_frame_alternates_channels_beyond_stereo() {
        let mut frame = [0.0f32; 4];
        write_frame(&mut frame, 0.25, -0.5);
        assert_eq!(frame, [0.25, -0.5, 0.25, -0.5]);
    }

    #[test]
    fn write_frame_saturates_integer_formats_at_full_scale() {
        let mut signed = [0i16; 2];
        write_frame(&mut signed, 1.5, -1.5);
        assert_eq!(signed, [i16::MAX, i16::MIN]);

        let mut unsigned = [0u16; 2];
        write_frame(&mut unsigned, 0.0, -1.0);
        assert!(unsigned[0].abs_diff(32_768) <= 1);
        assert_eq!(unsigned[1], 0);
    }

    #[test]
    fn write_frame_keeps_24_bit_full_scale_in_range() {
        const I24_MAX: i32 = (1 << 23) - 1;
        const I24_MIN: i32 = -(1 << 23);
        const U24_MAX: i32 = (1 << 24) - 1;

        let mut signed = [<cpal::I24 as cpal::Sample>::EQUILIBRIUM; 2];
        write_frame(&mut signed, 1.0, -1.0);
        assert_eq!(signed[0].inner(), I24_MAX);
        assert_eq!(signed[1].inner(), I24_MIN);

        let mut unsigned = [<cpal::U24 as cpal::Sample>::EQUILIBRIUM; 2];
        write_frame(&mut unsigned, 4.0, -4.0);
        assert_eq!(unsigned[0].inner(), U24_MAX);
        assert_eq!(unsigned[1].inner(), 0);
    }

    #[test]
    fn render_block_ignores_a_trailing_partial_frame() {
        let mut data = [9.0f32; 5];
        let mut left = [0.0f32; MAX_BLOCK];
        let mut right = [0.0f32; MAX_BLOCK];
        let mut render = |l: &mut [f32], r: &mut [f32]| {
            l.fill(0.25);
            r.fill(-0.25);
        };
        render_block(
            &mut data,
            2,
            &mut left,
            &mut right,
            &mut render,
            write_frame::<f32>,
        );
        assert_eq!(data, [0.25, -0.25, 0.25, -0.25, 9.0]);
    }

    #[test]
    fn only_xruns_skip_the_stream_error_count() {
        assert_eq!(stream_error_kind(cpal::ErrorKind::Xrun), None);
        assert_eq!(
            stream_error_kind(cpal::ErrorKind::StreamInvalidated),
            Some(StreamErrorKind::StreamInvalidated)
        );
        assert_eq!(
            stream_error_kind(cpal::ErrorKind::BackendError),
            Some(StreamErrorKind::Other)
        );
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
