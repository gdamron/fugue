//! Audio output backend abstraction and default cpal-based implementation.

use crate::SinkOutput;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

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

    /// Starts audio output with the given sample function.
    ///
    /// The sample function is called once per audio frame to produce samples.
    /// It will be called from the audio thread.
    fn start(
        &mut self,
        sample_fn: Box<dyn FnMut() -> SinkOutput + Send>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Stops audio output.
    fn stop(&mut self);
}

/// Default audio backend using the cpal library.
///
/// Sends audio to the system's default output device.
/// Supports F32, I16, and U16 sample formats.
pub struct AudioDriver {
    stream: Option<Stream>,
    sample_rate: u32,
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
        })
    }
}

impl AudioBackend for AudioDriver {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(
        &mut self,
        mut sample_fn: Box<dyn FnMut() -> SinkOutput + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let channels = config.channels() as usize;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                Self::build_stream::<f32>(&device, &config.into(), move |data: &mut [f32]| {
                    for frame in data.chunks_mut(channels) {
                        let output = sample_fn();
                        Self::write_frame_f32(frame, output);
                    }
                })?
            }
            cpal::SampleFormat::I16 => {
                Self::build_stream::<i16>(&device, &config.into(), move |data: &mut [i16]| {
                    for frame in data.chunks_mut(channels) {
                        let output = sample_fn();
                        Self::write_frame_i16(frame, output);
                    }
                })?
            }
            cpal::SampleFormat::U16 => {
                Self::build_stream::<u16>(&device, &config.into(), move |data: &mut [u16]| {
                    for frame in data.chunks_mut(channels) {
                        let output = sample_fn();
                        Self::write_frame_u16(frame, output);
                    }
                })?
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
}

impl AudioDriver {
    fn stereo_values(output: SinkOutput) -> (f32, f32) {
        (output.left.clamp(-1.0, 1.0), output.right.clamp(-1.0, 1.0))
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

    fn write_frame_f32(frame: &mut [f32], output: SinkOutput) {
        let (left, right) = Self::stereo_values(output);
        if frame.len() == 1 {
            frame[0] = (left + right) * 0.5;
            return;
        }
        Self::write_channels(frame, left, right);
    }

    fn write_frame_i16(frame: &mut [i16], output: SinkOutput) {
        let (left, right) = Self::stereo_values(output);
        if frame.len() == 1 {
            frame[0] = (((left + right) * 0.5) * i16::MAX as f32) as i16;
            return;
        }
        Self::write_channels(
            frame,
            (left * i16::MAX as f32) as i16,
            (right * i16::MAX as f32) as i16,
        );
    }

    fn write_frame_u16(frame: &mut [u16], output: SinkOutput) {
        let (left, right) = Self::stereo_values(output);
        if frame.len() == 1 {
            frame[0] = ((((left + right) * 0.5) + 1.0) * 0.5 * u16::MAX as f32) as u16;
            return;
        }
        Self::write_channels(
            frame,
            ((left + 1.0) * 0.5 * u16::MAX as f32) as u16,
            ((right + 1.0) * 0.5 * u16::MAX as f32) as u16,
        );
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        mut callback: impl FnMut(&mut [T]) + Send + 'static,
    ) -> Result<Stream, Box<dyn std::error::Error>>
    where
        T: cpal::Sample + cpal::SizedSample,
    {
        let stream = device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                callback(data);
            },
            |err| eprintln!("Stream error: {}", err),
            None,
        )?;

        Ok(stream)
    }
}
