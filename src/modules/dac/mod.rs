//! Audio output using the system's default audio device.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

/// Digital-to-Analog Converter that sends audio to the system output device.
///
/// Wraps the cpal library to provide cross-platform audio output.
/// Supports F32, I16, and U16 sample formats.
pub struct Dac {
    stream: Option<Stream>,
    sample_rate: u32,
}

impl Dac {
    /// Creates a new DAC using the system's default output device.
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

    /// Returns the sample rate of the output device.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Starts audio playback from the given sample function.
    ///
    /// The sample function is called once per audio frame to produce samples.
    /// Output is duplicated to all channels (mono to stereo conversion).
    pub fn start<F>(&mut self, mut sample_fn: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: FnMut() -> f32 + Send + 'static,
    {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let channels = config.channels() as usize;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                self.build_stream::<f32>(&device, &config.into(), move |data: &mut [f32]| {
                    for frame in data.chunks_mut(channels) {
                        let value = sample_fn().clamp(-1.0, 1.0);
                        for sample in frame.iter_mut() {
                            *sample = value;
                        }
                    }
                })?
            }
            cpal::SampleFormat::I16 => {
                self.build_stream::<i16>(&device, &config.into(), move |data: &mut [i16]| {
                    for frame in data.chunks_mut(channels) {
                        let value = (sample_fn().clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        for sample in frame.iter_mut() {
                            *sample = value;
                        }
                    }
                })?
            }
            cpal::SampleFormat::U16 => {
                self.build_stream::<u16>(&device, &config.into(), move |data: &mut [u16]| {
                    for frame in data.chunks_mut(channels) {
                        let value =
                            ((sample_fn().clamp(-1.0, 1.0) + 1.0) * 0.5 * u16::MAX as f32) as u16;
                        for sample in frame.iter_mut() {
                            *sample = value;
                        }
                    }
                })?
            }
            _ => return Err("Unsupported sample format".into()),
        };

        stream.play()?;
        self.stream = Some(stream);

        Ok(())
    }

    fn build_stream<T>(
        &self,
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

    /// Stops audio playback and releases the audio stream.
    pub fn stop(&mut self) {
        self.stream = None;
    }
}
