//! Thread-safe controls for the Mixer module.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

use super::MAX_CHANNELS;

/// Thread-safe controls for the Mixer module.
///
/// Controls use hierarchical keys for array access:
/// - `level.0` through `level.63` - Per-channel levels
/// - `pan.0` through `pan.63` - Per-channel pan positions (-1.0 to 1.0)
/// - `master` - Master output level
///
/// # Example
///
/// ```rust,ignore
/// let controls: MixerControls = handles.get("mixer.controls").unwrap();
///
/// // Adjust levels and panning in real-time
/// controls.set_level(0, 0.8);  // Channel 1 at 80%
/// controls.set_level(1, 0.5);  // Channel 2 at 50%
/// controls.set_pan(0, -0.25);  // Channel 1 slightly left
/// controls.set_pan(1, 0.25);   // Channel 2 slightly right
/// controls.set_master(0.7);    // Master at 70%
/// ```
#[derive(Clone)]
pub struct MixerControls {
    pub(crate) levels: [AtomicF32; MAX_CHANNELS],
    pub(crate) pans: [AtomicF32; MAX_CHANNELS],
    pub(crate) master: AtomicF32,
    pub(crate) channels: usize,
}

impl MixerControls {
    /// Creates new mixer controls with the given number of channels.
    ///
    /// All levels default to 1.0 (unity gain).
    pub fn new(channels: usize) -> Self {
        Self {
            levels: std::array::from_fn(|_| AtomicF32::new(1.0)),
            pans: std::array::from_fn(|_| AtomicF32::new(0.0)),
            master: AtomicF32::new(1.0),
            channels: channels.clamp(1, MAX_CHANNELS),
        }
    }

    /// Creates new mixer controls with specified initial levels and pan positions.
    pub fn new_with_config(
        channels: usize,
        initial_levels: &[f32],
        initial_pans: &[f32],
        master: f32,
    ) -> Self {
        let levels: [AtomicF32; MAX_CHANNELS] = std::array::from_fn(|i| {
            AtomicF32::new(
                initial_levels
                    .get(i)
                    .copied()
                    .unwrap_or(1.0)
                    .clamp(0.0, 2.0),
            )
        });
        let pans: [AtomicF32; MAX_CHANNELS] = std::array::from_fn(|i| {
            AtomicF32::new(initial_pans.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0))
        });
        Self {
            levels,
            pans,
            master: AtomicF32::new(master.clamp(0.0, 2.0)),
            channels: channels.clamp(1, MAX_CHANNELS),
        }
    }

    /// Returns the number of channels.
    pub fn channel_count(&self) -> usize {
        self.channels
    }

    /// Gets the level for a specific channel (0-indexed).
    pub fn level(&self, channel: usize) -> f32 {
        if channel < self.channels {
            self.levels[channel].load()
        } else {
            0.0
        }
    }

    /// Sets the level for a specific channel (0-indexed).
    pub fn set_level(&self, channel: usize, level: f32) {
        if channel < self.channels {
            self.levels[channel].store(level.clamp(0.0, 2.0));
        }
    }

    /// Gets the pan for a specific channel (0-indexed).
    pub fn pan(&self, channel: usize) -> f32 {
        if channel < self.channels {
            self.pans[channel].load()
        } else {
            0.0
        }
    }

    /// Sets the pan for a specific channel (0-indexed).
    pub fn set_pan(&self, channel: usize, pan: f32) {
        if channel < self.channels {
            self.pans[channel].store(pan.clamp(-1.0, 1.0));
        }
    }

    /// Gets the master level.
    pub fn master(&self) -> f32 {
        self.master.load()
    }

    /// Sets the master level.
    pub fn set_master(&self, level: f32) {
        self.master.store(level.clamp(0.0, 2.0));
    }
}

impl ControlSurface for MixerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let mut controls = Vec::with_capacity(self.channels * 2 + 1);
        for i in 0..self.channels {
            controls.push(
                ControlMeta::number(format!("level.{}", i), format!("Channel {} level", i + 1))
                    .with_range(0.0, 2.0)
                    .with_default(self.level(i)),
            );
            controls.push(
                ControlMeta::number(format!("pan.{}", i), format!("Channel {} pan", i + 1))
                    .with_range(-1.0, 1.0)
                    .with_default(self.pan(i)),
            );
        }
        controls.push(
            ControlMeta::number("master", "Master output level")
                .with_range(0.0, 2.0)
                .with_default(self.master()),
        );
        controls
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        if key == "master" {
            return Ok(self.master().into());
        }

        if let Some(rest) = key.strip_prefix("level.") {
            if let Ok(index) = rest.parse::<usize>() {
                if index < self.channels {
                    return Ok(self.level(index).into());
                }
            }
        }

        if let Some(rest) = key.strip_prefix("pan.") {
            if let Ok(index) = rest.parse::<usize>() {
                if index < self.channels {
                    return Ok(self.pan(index).into());
                }
            }
        }

        Err(format!("Unknown control: {}", key))
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let value = value.as_number()?;
        if key == "master" {
            self.set_master(value);
            return Ok(());
        }

        if let Some(rest) = key.strip_prefix("level.") {
            if let Ok(index) = rest.parse::<usize>() {
                if index < self.channels {
                    self.set_level(index, value);
                    return Ok(());
                }
            }
        }

        if let Some(rest) = key.strip_prefix("pan.") {
            if let Ok(index) = rest.parse::<usize>() {
                if index < self.channels {
                    self.set_pan(index, value);
                    return Ok(());
                }
            }
        }

        Err(format!("Unknown control: {}", key))
    }
}
