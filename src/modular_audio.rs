use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

use crate::signal::AudioSignal;
use crate::module::Generator;

/// DAC (Digital-to-Analog Converter) - the output node that sends audio to speakers
/// In Eurorack terms, this is like the audio output jack
pub struct Dac {
    stream: Option<Stream>,
    sample_rate: u32,
}

impl Dac {
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

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Start playing audio from the given generator
    /// The generator should output AudioSignal samples
    pub fn start<G>(&mut self, mut generator: G) -> Result<(), Box<dyn std::error::Error>>
    where
        G: Generator<AudioSignal> + Send + 'static,
    {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let channels = config.channels() as usize;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => self.build_stream::<f32>(
                &device,
                &config.into(),
                move |data: &mut [f32]| {
                    // Process once per frame (not per sample)
                    for frame in data.chunks_mut(channels) {
                        generator.process();
                        let audio = generator.output();
                        let value = audio.value.clamp(-1.0, 1.0);
                        // Write same value to all channels (mono -> stereo)
                        for sample in frame.iter_mut() {
                            *sample = value;
                        }
                    }
                },
            )?,
            cpal::SampleFormat::I16 => self.build_stream::<i16>(
                &device,
                &config.into(),
                move |data: &mut [i16]| {
                    // Process once per frame (not per sample)
                    for frame in data.chunks_mut(channels) {
                        generator.process();
                        let audio = generator.output();
                        let value = (audio.value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        // Write same value to all channels (mono -> stereo)
                        for sample in frame.iter_mut() {
                            *sample = value;
                        }
                    }
                },
            )?,
            cpal::SampleFormat::U16 => self.build_stream::<u16>(
                &device,
                &config.into(),
                move |data: &mut [u16]| {
                    // Process once per frame (not per sample)
                    for frame in data.chunks_mut(channels) {
                        generator.process();
                        let audio = generator.output();
                        let value = ((audio.value.clamp(-1.0, 1.0) + 1.0) * 0.5 * u16::MAX as f32) as u16;
                        // Write same value to all channels (mono -> stereo)
                        for sample in frame.iter_mut() {
                            *sample = value;
                        }
                    }
                },
            )?,
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

    /// Stop audio playback
    pub fn stop(&mut self) {
        self.stream = None;
    }
}

