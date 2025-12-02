use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};

use crate::time::{Clock, Tempo};
use crate::synthesis::Oscillator;
use crate::sequencer::MelodyGenerator;

pub struct AudioEngine {
    stream: Option<Stream>,
    sample_rate: u32,
}

impl AudioEngine {
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

    #[allow(unused_assignments)]
    pub fn start_melody(
        &mut self,
        mut melody_gen: MelodyGenerator,
        tempo: Tempo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device available")?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;

        let mut clock = Clock::new(sample_rate, tempo);
        let mut oscillator = Oscillator::new(sample_rate, melody_gen.params().get_oscillator_type());

        let mut current_note = melody_gen.next_note();
        oscillator.set_frequency(current_note.frequency());

        let mut samples_since_note = 0u64;
        let melody_params = melody_gen.params().clone();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => self.build_stream::<f32>(
                &device,
                &config.into(),
                move |data: &mut [f32]| {
                    for sample in data.iter_mut() {
                        let note_duration = *melody_params.note_duration.lock().unwrap();
                        let samples_per_note =
                            (clock.tempo().samples_per_beat(sample_rate) * note_duration as f64) as u64;

                        if samples_since_note >= samples_per_note {
                            current_note = melody_gen.next_note();
                            let osc_type = melody_params.get_oscillator_type();
                            oscillator.set_type(osc_type);
                            oscillator.set_frequency(current_note.frequency());
                            samples_since_note = 0;
                        }

                        let envelope = if samples_since_note < samples_per_note / 10 {
                            samples_since_note as f32 / (samples_per_note as f32 / 10.0)
                        } else if samples_since_note > samples_per_note * 9 / 10 {
                            1.0 - ((samples_since_note - samples_per_note * 9 / 10) as f32
                                / (samples_per_note as f32 / 10.0))
                        } else {
                            1.0
                        };

                        *sample = oscillator.next_sample() * envelope * 0.15;

                        clock.tick();
                        samples_since_note += 1;
                    }
                },
            )?,
            cpal::SampleFormat::I16 => self.build_stream::<i16>(
                &device,
                &config.into(),
                move |data: &mut [i16]| {
                    for sample in data.iter_mut() {
                        let note_duration = *melody_params.note_duration.lock().unwrap();
                        let samples_per_note =
                            (clock.tempo().samples_per_beat(sample_rate) * note_duration as f64) as u64;

                        if samples_since_note >= samples_per_note {
                            current_note = melody_gen.next_note();
                            let osc_type = melody_params.get_oscillator_type();
                            oscillator.set_type(osc_type);
                            oscillator.set_frequency(current_note.frequency());
                            samples_since_note = 0;
                        }

                        let envelope = if samples_since_note < samples_per_note / 10 {
                            samples_since_note as f32 / (samples_per_note as f32 / 10.0)
                        } else if samples_since_note > samples_per_note * 9 / 10 {
                            1.0 - ((samples_since_note - samples_per_note * 9 / 10) as f32
                                / (samples_per_note as f32 / 10.0))
                        } else {
                            1.0
                        };

                        let value = (oscillator.next_sample() * envelope * 0.15 * i16::MAX as f32) as i16;
                        *sample = value;

                        clock.tick();
                        samples_since_note += 1;
                    }
                },
            )?,
            cpal::SampleFormat::U16 => self.build_stream::<u16>(
                &device,
                &config.into(),
                move |data: &mut [u16]| {
                    for sample in data.iter_mut() {
                        let note_duration = *melody_params.note_duration.lock().unwrap();
                        let samples_per_note =
                            (clock.tempo().samples_per_beat(sample_rate) * note_duration as f64) as u64;

                        if samples_since_note >= samples_per_note {
                            current_note = melody_gen.next_note();
                            let osc_type = melody_params.get_oscillator_type();
                            oscillator.set_type(osc_type);
                            oscillator.set_frequency(current_note.frequency());
                            samples_since_note = 0;
                        }

                        let envelope = if samples_since_note < samples_per_note / 10 {
                            samples_since_note as f32 / (samples_per_note as f32 / 10.0)
                        } else if samples_since_note > samples_per_note * 9 / 10 {
                            1.0 - ((samples_since_note - samples_per_note * 9 / 10) as f32
                                / (samples_per_note as f32 / 10.0))
                        } else {
                            1.0
                        };

                        let value = ((oscillator.next_sample() * envelope * 0.15 + 1.0) * 0.5 * u16::MAX as f32) as u16;
                        *sample = value;

                        clock.tick();
                        samples_since_note += 1;
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
        config: &StreamConfig,
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
