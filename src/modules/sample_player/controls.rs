//! Thread-safe controls for the SamplePlayer module.

use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::control_request::{
    scheduled_bool, scheduled_number, try_scheduled, LegacyRefusal, TableValue,
};
use crate::modules::sample_loading::{load_cached_sample, resolve_source, SampleData};
use crate::{ControlMeta, ControlSurface, ControlValue};

#[derive(Clone)]
pub struct SamplePlayerControls {
    pub(crate) shared: Arc<Mutex<SamplePlayerShared>>,
    pitch_ratio: AtomicF32,
    time_ratio: AtomicF32,
    sample_rate: u32,
    /// Fixed at build time from the `mode` config key. In elastic mode
    /// `pitch_ratio` is pitch-only and `time_ratio` speed-only; in classic
    /// mode `pitch_ratio` keeps its coupled speed+pitch meaning.
    elastic: bool,
}

pub(crate) struct SamplePlayerShared {
    pub(crate) asset: String,
    pub(crate) play: bool,
    pub(crate) loop_enabled: bool,
    pub(crate) play_trigger: u64,
    pub(crate) pending_sample: Option<Arc<SampleData>>,
}

impl SamplePlayerShared {
    /// Starts or stops playback; starting (again) retriggers it.
    fn set_play(&mut self, play: bool) {
        self.play = play;
        if play {
            self.play_trigger = self.play_trigger.wrapping_add(1);
        }
    }
}

impl SamplePlayerControls {
    pub fn new(
        sample_rate: u32,
        asset: Option<&str>,
        play: Option<bool>,
        loop_enabled: Option<bool>,
    ) -> Result<Self, String> {
        Self::with_mode(sample_rate, asset, play, loop_enabled, false)
    }

    pub fn with_mode(
        sample_rate: u32,
        asset: Option<&str>,
        play: Option<bool>,
        loop_enabled: Option<bool>,
        elastic: bool,
    ) -> Result<Self, String> {
        let controls = Self {
            shared: Arc::new(Mutex::new(SamplePlayerShared {
                asset: String::new(),
                play: false,
                loop_enabled: false,
                play_trigger: 0,
                pending_sample: None,
            })),
            pitch_ratio: AtomicF32::new(1.0),
            time_ratio: AtomicF32::new(1.0),
            sample_rate,
            elastic,
        };

        if let Some(asset) = asset {
            if !asset.is_empty() {
                controls.set_asset(asset)?;
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

    pub fn asset(&self) -> String {
        self.shared.lock().unwrap().asset.clone()
    }

    pub fn set_asset(&self, asset: &str) -> Result<(), String> {
        let target = resolve_source(asset)?;
        let sample = load_cached_sample(&target, self.sample_rate)?;
        if self.elastic {
            // Elastic playback needs the per-asset analysis; computing it
            // here keeps the allocation off the audio thread (the reader
            // finds it on the SampleData via the lock-free accessor).
            sample.elastic_analysis();
        }
        let mut shared = self.shared.lock().unwrap();
        // The authored ref stays the control value, so a saved document keeps
        // the portable form rather than this machine's cache path.
        shared.asset = asset.to_string();
        shared.pending_sample = Some(sample);
        Ok(())
    }

    pub fn play(&self) -> bool {
        self.shared.lock().unwrap().play
    }

    pub fn set_play(&self, play: bool) {
        self.shared.lock().unwrap().set_play(play);
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

    pub fn time_ratio(&self) -> f32 {
        self.time_ratio.load()
    }

    pub fn set_time_ratio(&self, time_ratio: f32) {
        self.time_ratio.store(time_ratio.max(1e-4));
    }

    pub(crate) fn is_elastic(&self) -> bool {
        self.elastic
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl ControlSurface for SamplePlayerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let pitch_description = if self.elastic {
            "Pitch ratio (1.0 = native, 2.0 = up an octave) without changing speed"
        } else {
            "Playback speed / pitch ratio (1.0 = native, 2.0 = up an octave)"
        };
        let mut controls = vec![
            ControlMeta::string(
                "asset",
                "Audio sample path, https URL, or package ref like \
                 'fugue.drums.808@1.2.0:kick/long.wav' (WAV or FLAC)",
            )
            .with_default(self.asset()),
            ControlMeta::boolean("play", "Start or stop sample playback", self.play()),
            ControlMeta::boolean("loop", "Loop playback when enabled", self.loop_enabled()),
            ControlMeta::number("pitch_ratio", pitch_description)
                .with_range(0.25, 4.0)
                .with_default(self.pitch_ratio()),
        ];
        if self.elastic {
            controls.push(
                ControlMeta::number(
                    "time_ratio",
                    "Playback speed ratio (1.0 = native, 2.0 = double speed) \
                     without changing pitch",
                )
                .with_range(0.25, 4.0)
                .with_default(self.time_ratio()),
            );
        }
        controls
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "asset" => Ok(self.asset().into()),
            "play" => Ok(self.play().into()),
            "loop" => Ok(self.loop_enabled().into()),
            "pitch_ratio" => Ok(self.pitch_ratio().into()),
            "time_ratio" if self.elastic => Ok(self.time_ratio().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "asset" => self.set_asset(value.as_string()?)?,
            "play" => self.set_play(value.as_bool()?),
            "loop" => self.set_loop_enabled(value.as_bool()?),
            "pitch_ratio" => self.set_pitch_ratio(value.as_number()?),
            "time_ratio" if self.elastic => self.set_time_ratio(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }

    /// `play` and `loop` take the shared state's lock without waiting: a
    /// control thread holding it refuses the write as busy.
    #[allow(private_interfaces)]
    fn set_scheduled(&self, key: &str, value: TableValue) -> Result<(), LegacyRefusal> {
        match key {
            "play" => try_scheduled(&self.shared)?.set_play(scheduled_bool(value)?),
            "loop" => try_scheduled(&self.shared)?.loop_enabled = scheduled_bool(value)?,
            "pitch_ratio" => self.set_pitch_ratio(scheduled_number(value)?),
            "time_ratio" if self.elastic => self.set_time_ratio(scheduled_number(value)?),
            _ => return Err(LegacyRefusal::Unsupported),
        }
        Ok(())
    }

    #[allow(private_interfaces)]
    fn get_scheduled(&self, key: &str) -> Result<f32, LegacyRefusal> {
        match key {
            "pitch_ratio" => Ok(self.pitch_ratio()),
            "time_ratio" if self.elastic => Ok(self.time_ratio()),
            _ => Err(LegacyRefusal::Unsupported),
        }
    }

    /// `play` starts the sample over (or stops it): an event, as a trigger.
    fn scheduled_event(&self, key: &str) -> bool {
        key == "play"
    }
}

#[cfg(test)]
mod scheduled_tests {
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::control_request::while_held;

    #[test]
    fn a_scheduled_write_to_held_playback_state_is_refused_without_waiting() {
        let controls = SamplePlayerControls::with_mode(48_000, None, None, None, true).unwrap();
        let on = TableValue::Bool(true);
        let (written, allocs, frees) = while_held(&controls.shared, || {
            allocator_events(|| {
                let flags = ["play", "loop"].map(|key| controls.set_scheduled(key, on));
                (
                    flags,
                    controls.set_scheduled("time_ratio", TableValue::Number(2.0)),
                )
            })
        });
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(written.0, [Err(LegacyRefusal::Busy); 2]);
        assert_eq!(written.1, Ok(()), "an atomic control needs no lock");
        assert_eq!(controls.time_ratio(), 2.0);
        assert!(!controls.play() && !controls.loop_enabled());
        // Released, starting playback retriggers it, as the setter does.
        let trigger = controls.shared.lock().unwrap().play_trigger;
        assert_eq!(controls.set_scheduled("play", on), Ok(()));
        assert!(controls.play());
        assert_eq!(controls.shared.lock().unwrap().play_trigger, trigger + 1);
    }
}
