//! Thread-safe controls for the SamplePlayer module.

use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::modules::sample_loading::{load_cached_sample, resolve_source, SampleData};
use crate::{ControlMeta, ControlSurface, ControlValue};

#[derive(Clone)]
pub struct SamplePlayerControls {
    pub(crate) shared: Arc<Mutex<SamplePlayerShared>>,
    pitch_ratio: AtomicF32,
    sample_rate: u32,
}

pub(crate) struct SamplePlayerShared {
    pub(crate) source: String,
    pub(crate) play: bool,
    pub(crate) loop_enabled: bool,
    pub(crate) play_trigger: u64,
    pub(crate) pending_sample: Option<Arc<SampleData>>,
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
            pitch_ratio: AtomicF32::new(1.0),
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
        let target = resolve_source(source)?;
        let sample = load_cached_sample(&target, self.sample_rate)?;
        let mut shared = self.shared.lock().unwrap();
        // The authored ref stays the control value, so a saved document keeps
        // the portable form rather than this machine's cache path.
        shared.source = source.to_string();
        shared.pending_sample = Some(sample);
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

    pub fn pitch_ratio(&self) -> f32 {
        self.pitch_ratio.load()
    }

    pub fn set_pitch_ratio(&self, pitch_ratio: f32) {
        // Clamp to a small positive floor so the read head always advances
        // forward; a zero or negative ratio would stall or reverse playback.
        self.pitch_ratio.store(pitch_ratio.max(1e-4));
    }
}

impl ControlSurface for SamplePlayerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::string(
                "source",
                "Audio sample path, https URL, or package ref like \
                 'fugue.drums.808@1.2.0:kick/long.wav' (WAV or FLAC)",
            )
            .with_default(self.source()),
            ControlMeta::boolean("play", "Start or stop sample playback", self.play()),
            ControlMeta::boolean("loop", "Loop playback when enabled", self.loop_enabled()),
            ControlMeta::number(
                "pitch_ratio",
                "Playback speed / pitch ratio (1.0 = native, 2.0 = up an octave)",
            )
            .with_range(0.25, 4.0)
            .with_default(self.pitch_ratio()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "source" => Ok(self.source().into()),
            "play" => Ok(self.play().into()),
            "loop" => Ok(self.loop_enabled().into()),
            "pitch_ratio" => Ok(self.pitch_ratio().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "source" => self.set_source(value.as_string()?)?,
            "play" => self.set_play(value.as_bool()?),
            "loop" => self.set_loop_enabled(value.as_bool()?),
            "pitch_ratio" => self.set_pitch_ratio(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
