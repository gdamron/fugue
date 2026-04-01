//! Thread-safe controls for the SamplePlayer module.

use std::io::Read;
use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

#[derive(Clone)]
pub struct SamplePlayerControls {
    pub(crate) shared: Arc<Mutex<SamplePlayerShared>>,
    sample_rate: u32,
}

pub(crate) struct SamplePlayerShared {
    pub(crate) source: String,
    pub(crate) play: bool,
    pub(crate) loop_enabled: bool,
    pub(crate) play_trigger: u64,
    pub(crate) pending_sample: Option<Arc<SampleData>>,
}

pub(crate) struct SampleData {
    pub(crate) left: Vec<f32>,
    pub(crate) right: Vec<f32>,
}

impl SampleData {
    pub(crate) fn len(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    fn from_interleaved(
        channels: usize,
        sample_rate: u32,
        target_sample_rate: u32,
        samples: Vec<f32>,
    ) -> Self {
        if channels == 0 {
            return Self {
                left: Vec::new(),
                right: Vec::new(),
            };
        }

        let frames = samples.len() / channels;
        let mut left = Vec::with_capacity(frames);
        let mut right = Vec::with_capacity(frames);

        for frame in samples.chunks(channels) {
            let l = frame.first().copied().unwrap_or(0.0);
            let r = frame.get(1).copied().unwrap_or(l);
            left.push(l);
            right.push(r);
        }

        if sample_rate == target_sample_rate || left.is_empty() {
            return Self { left, right };
        }

        Self {
            left: resample_channel(&left, sample_rate, target_sample_rate),
            right: resample_channel(&right, sample_rate, target_sample_rate),
        }
    }
}

impl SamplePlayerControls {
    pub fn new(
        sample_rate: u32,
        source: Option<&str>,
        play: Option<bool>,
        loop_enabled: Option<bool>,
    ) -> Result<Self, String> {
        let controls = Self {
            shared: Arc::new(Mutex::new(SamplePlayerShared {
                source: String::new(),
                play: false,
                loop_enabled: false,
                play_trigger: 0,
                pending_sample: None,
            })),
            sample_rate,
        };

        if let Some(source) = source {
            if !source.is_empty() {
                controls.set_source(source)?;
            }
        }

        if let Some(play) = play {
            controls.set_play(play);
        }

        if let Some(loop_enabled) = loop_enabled {
            controls.set_loop_enabled(loop_enabled);
        }

        Ok(controls)
    }

    pub fn source(&self) -> String {
        self.shared.lock().unwrap().source.clone()
    }

    pub fn set_source(&self, source: &str) -> Result<(), String> {
        let sample = load_sample(source, self.sample_rate)?;
        let mut shared = self.shared.lock().unwrap();
        shared.source = source.to_string();
        shared.pending_sample = Some(Arc::new(sample));
        Ok(())
    }

    pub fn play(&self) -> bool {
        self.shared.lock().unwrap().play
    }

    pub fn set_play(&self, play: bool) {
        let mut shared = self.shared.lock().unwrap();
        shared.play = play;
        if play {
            shared.play_trigger = shared.play_trigger.wrapping_add(1);
        }
    }

    pub fn loop_enabled(&self) -> bool {
        self.shared.lock().unwrap().loop_enabled
    }

    pub fn set_loop_enabled(&self, loop_enabled: bool) {
        self.shared.lock().unwrap().loop_enabled = loop_enabled;
    }
}

impl ControlSurface for SamplePlayerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::string("source", "Audio sample path or https URL")
                .with_default(self.source()),
            ControlMeta::boolean("play", "Start or stop sample playback", self.play()),
            ControlMeta::boolean("loop", "Loop playback when enabled", self.loop_enabled()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "source" => Ok(self.source().into()),
            "play" => Ok(self.play().into()),
            "loop" => Ok(self.loop_enabled().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "source" => self.set_source(value.as_string()?),
            "play" => Ok(self.set_play(value.as_bool()?)),
            "loop" => Ok(self.set_loop_enabled(value.as_bool()?)),
            _ => return Err(format!("Unknown control: {}", key)),
        }
    }
}

fn load_sample(source: &str, target_sample_rate: u32) -> Result<SampleData, String> {
    let (reader, remote) = open_source(source)?;
    let wav =
        hound::WavReader::new(reader).map_err(|err| format!("Failed to decode WAV: {}", err))?;
    let spec = wav.spec();
    let channels = spec.channels.max(1) as usize;
    let sample_rate = spec.sample_rate;
    let samples = decode_samples(wav)?;
    let data = SampleData::from_interleaved(channels, sample_rate, target_sample_rate, samples);

    if data.len() == 0 {
        let location = if remote { "URL" } else { "file" };
        return Err(format!("Decoded empty sample from {}", location));
    }

    Ok(data)
}

fn open_source(source: &str) -> Result<(Box<dyn Read>, bool), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = source;
        Err("Sample loading is not available on wasm32".to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if source.starts_with("https://") {
            let response = ureq::get(source)
                .call()
                .map_err(|err| format!("Failed to download sample: {}", err))?;
            return Ok((Box::new(response.into_reader()), true));
        }

        if source.starts_with("http://") {
            return Err("Only https:// URLs are supported".to_string());
        }

        let file = std::fs::File::open(source)
            .map_err(|err| format!("Failed to open sample '{}': {}", source, err))?;
        Ok((Box::new(file), false))
    }
}

fn decode_samples<R: Read>(mut wav: hound::WavReader<R>) -> Result<Vec<f32>, String> {
    let spec = wav.spec();
    match spec.sample_format {
        hound::SampleFormat::Float => wav
            .samples::<f32>()
            .map(|sample| sample.map_err(|err| err.to_string()))
            .collect(),
        hound::SampleFormat::Int => {
            let shift = spec.bits_per_sample.saturating_sub(1) as u32;
            let scale = (1_i64 << shift) as f32;
            wav.samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| (value as f32 / scale).clamp(-1.0, 1.0))
                        .map_err(|err| err.to_string())
                })
                .collect()
        }
    }
}

fn resample_channel(input: &[f32], sample_rate: u32, target_sample_rate: u32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }

    let target_len = ((input.len() as f64 * target_sample_rate as f64) / sample_rate as f64)
        .round()
        .max(1.0) as usize;
    let ratio = sample_rate as f64 / target_sample_rate as f64;
    let mut output = Vec::with_capacity(target_len);

    for index in 0..target_len {
        let source_pos = index as f64 * ratio;
        let base = source_pos.floor() as usize;
        let frac = (source_pos - base as f64) as f32;
        let a = input[base.min(input.len() - 1)];
        let b = input[(base + 1).min(input.len() - 1)];
        output.push(a + (b - a) * frac);
    }

    output
}
