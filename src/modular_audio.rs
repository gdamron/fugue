use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

use crate::sequencer::{NoteSignal, MelodyParams};
use crate::module::Generator;
use crate::synthesis::Oscillator;

/// ModularAudioEngine - plays a modular voice chain through the audio device
pub struct ModularAudioEngine {
    stream: Option<Stream>,
    sample_rate: u32,
}

impl ModularAudioEngine {
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

    /// Start a voice generator that combines note generation with oscillator
    pub fn start_voice<G>(
        &mut self,
        mut voice_gen: G,
        melody_params: MelodyParams,
    ) -> Result<(), Box<dyn std::error::Error>>
    where
        G: Generator<NoteSignal> + Send + 'static,
    {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;

        let mut oscillator = Oscillator::new(sample_rate, melody_params.get_oscillator_type());

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => self.build_stream::<f32>(
                &device,
                &config.into(),
                move |data: &mut [f32]| {
                    for sample in data.iter_mut() {
                        // Process the voice generator
                        voice_gen.process();
                        let note_signal = voice_gen.output();

                        // Update oscillator type if changed
                        let osc_type = melody_params.get_oscillator_type();
                        oscillator.set_type(osc_type);
                        oscillator.set_frequency(note_signal.frequency.hz);

                        // Generate audio with envelope
                        let audio_sample = oscillator.output();
                        *sample = audio_sample.value * note_signal.gate.velocity * 0.15;
                    }
                },
            )?,
            cpal::SampleFormat::I16 => self.build_stream::<i16>(
                &device,
                &config.into(),
                move |data: &mut [i16]| {
                    for sample in data.iter_mut() {
                        voice_gen.process();
                        let note_signal = voice_gen.output();

                        let osc_type = melody_params.get_oscillator_type();
                        oscillator.set_type(osc_type);
                        oscillator.set_frequency(note_signal.frequency.hz);

                        let audio_sample = oscillator.output();
                        let value = (audio_sample.value * note_signal.gate.velocity * 0.15 * i16::MAX as f32) as i16;
                        *sample = value;
                    }
                },
            )?,
            cpal::SampleFormat::U16 => self.build_stream::<u16>(
                &device,
                &config.into(),
                move |data: &mut [u16]| {
                    for sample in data.iter_mut() {
                        voice_gen.process();
                        let note_signal = voice_gen.output();

                        let osc_type = melody_params.get_oscillator_type();
                        oscillator.set_type(osc_type);
                        oscillator.set_frequency(note_signal.frequency.hz);

                        let audio_sample = oscillator.output();
                        let value = ((audio_sample.value * note_signal.gate.velocity * 0.15 + 1.0) * 0.5 * u16::MAX as f32) as u16;
                        *sample = value;
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

    pub fn stop(&mut self) {
        self.stream = None;
    }
}
