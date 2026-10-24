//! A legacy control surface for the scheduler's tests of the legacy path:
//! `level.N` controls held in atomics, written the moment a schedule
//! writes them, as surfaces were before modules declared their controls.

use crate::atomic::AtomicF32;
use crate::control_request::{scheduled_index, scheduled_number, LegacyRefusal, TableValue};
use crate::{ControlMeta, ControlSurface, ControlValue};

/// Levels from 0 to 2, unity at first, one per channel. A clone shares
/// them.
#[derive(Clone)]
pub(crate) struct LegacyLevels {
    levels: Vec<AtomicF32>,
}

impl LegacyLevels {
    pub(crate) fn new(channels: usize) -> Self {
        let levels = (0..channels).map(|_| AtomicF32::new(1.0)).collect();
        Self { levels }
    }

    pub(crate) fn level(&self, channel: usize) -> f32 {
        self.levels[channel].load()
    }

    pub(crate) fn set_level(&self, channel: usize, level: f32) {
        self.levels[channel].store(level.clamp(0.0, 2.0));
    }

    /// The channel `key` names, parsed without allocating.
    fn channel(&self, key: &str) -> Option<usize> {
        scheduled_index(key, "level.").filter(|&channel| channel < self.levels.len())
    }

    fn unknown(key: &str) -> String {
        format!("Unknown control: {key}")
    }
}

impl ControlSurface for LegacyLevels {
    fn controls(&self) -> Vec<ControlMeta> {
        (0..self.levels.len())
            .map(|channel| {
                ControlMeta::number(format!("level.{channel}"), "Level")
                    .with_range(0.0, 2.0)
                    .with_default(self.level(channel))
            })
            .collect()
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        let channel = self.channel(key).ok_or_else(|| Self::unknown(key))?;
        Ok(self.level(channel).into())
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let channel = self.channel(key).ok_or_else(|| Self::unknown(key))?;
        self.set_level(channel, value.as_number()?);
        Ok(())
    }

    #[allow(private_interfaces)]
    fn set_scheduled(&self, key: &str, value: TableValue) -> Result<(), LegacyRefusal> {
        let value = scheduled_number(value)?;
        let channel = self.channel(key).ok_or(LegacyRefusal::Unsupported)?;
        self.set_level(channel, value);
        Ok(())
    }

    #[allow(private_interfaces)]
    fn get_scheduled(&self, key: &str) -> Result<f32, LegacyRefusal> {
        let channel = self.channel(key).ok_or(LegacyRefusal::Unsupported)?;
        Ok(self.level(channel))
    }
}
