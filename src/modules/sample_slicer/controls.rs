//! Live controls for `sample_slicer`, exposed only in elastic mode.
//!
//! Classic slicing has no control surface: playback is fully determined by
//! the trigger/slice inputs. Elastic mode adds the shared time/pitch ratios
//! so slices can be tempo-fit without chipmunking.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

#[derive(Clone)]
pub struct SampleSlicerControls {
    time_ratio: AtomicF32,
    pitch_ratio: AtomicF32,
}

impl SampleSlicerControls {
    pub fn new() -> Self {
        Self {
            time_ratio: AtomicF32::new(1.0),
            pitch_ratio: AtomicF32::new(1.0),
        }
    }

    pub fn time_ratio(&self) -> f32 {
        self.time_ratio.load()
    }

    pub fn set_time_ratio(&self, time_ratio: f32) {
        // Clamp to a small positive floor so the read head always advances
        // forward; a zero or negative ratio would stall playback.
        self.time_ratio.store(time_ratio.max(1e-4));
    }

    pub fn pitch_ratio(&self) -> f32 {
        self.pitch_ratio.load()
    }

    pub fn set_pitch_ratio(&self, pitch_ratio: f32) {
        self.pitch_ratio.store(pitch_ratio.max(1e-4));
    }
}

impl Default for SampleSlicerControls {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlSurface for SampleSlicerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number(
                "time_ratio",
                "Slice playback speed ratio (1.0 = native, 2.0 = double speed) \
                 without changing pitch",
            )
            .with_range(0.25, 4.0)
            .with_default(self.time_ratio()),
            ControlMeta::number(
                "pitch_ratio",
                "Slice pitch ratio (1.0 = native, 2.0 = up an octave) \
                 without changing speed",
            )
            .with_range(0.25, 4.0)
            .with_default(self.pitch_ratio()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "time_ratio" => Ok(self.time_ratio().into()),
            "pitch_ratio" => Ok(self.pitch_ratio().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "time_ratio" => self.set_time_ratio(value.as_number()?),
            "pitch_ratio" => self.set_pitch_ratio(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
