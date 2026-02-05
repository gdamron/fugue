//! Thread-safe controls for the Mixer module.

use std::sync::{Arc, Mutex};

use super::MAX_CHANNELS;

/// Thread-safe controls for the Mixer module.
///
/// Controls use hierarchical keys for array access:
/// - `level.0` through `level.7` - Per-channel levels
/// - `master` - Master output level
///
/// # Example
///
/// ```rust,ignore
/// let controls: MixerControls = handles.get("mixer.controls").unwrap();
///
/// // Adjust levels in real-time
/// controls.set_level(0, 0.8);  // Channel 1 at 80%
/// controls.set_level(1, 0.5);  // Channel 2 at 50%
/// controls.set_master(0.7);    // Master at 70%
/// ```
#[derive(Clone)]
pub struct MixerControls {
    pub(crate) levels: Arc<Mutex<[f32; MAX_CHANNELS]>>,
    pub(crate) master: Arc<Mutex<f32>>,
    pub(crate) channels: usize,
}

impl MixerControls {
    /// Creates new mixer controls with the given number of channels.
    ///
    /// All levels default to 1.0 (unity gain).
    pub fn new(channels: usize) -> Self {
        Self {
            levels: Arc::new(Mutex::new([1.0; MAX_CHANNELS])),
            master: Arc::new(Mutex::new(1.0)),
            channels: channels.clamp(1, MAX_CHANNELS),
        }
    }

    /// Creates new mixer controls with specified initial levels.
    pub fn new_with_levels(channels: usize, initial_levels: &[f32], master: f32) -> Self {
        let mut levels = [1.0; MAX_CHANNELS];
        for (i, &level) in initial_levels.iter().enumerate() {
            if i < MAX_CHANNELS {
                levels[i] = level.clamp(0.0, 2.0);
            }
        }
        Self {
            levels: Arc::new(Mutex::new(levels)),
            master: Arc::new(Mutex::new(master.clamp(0.0, 2.0))),
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
            self.levels.lock().unwrap()[channel]
        } else {
            0.0
        }
    }

    /// Sets the level for a specific channel (0-indexed).
    pub fn set_level(&self, channel: usize, level: f32) {
        if channel < self.channels {
            self.levels.lock().unwrap()[channel] = level.clamp(0.0, 2.0);
        }
    }

    /// Gets the master level.
    pub fn master(&self) -> f32 {
        *self.master.lock().unwrap()
    }

    /// Sets the master level.
    pub fn set_master(&self, level: f32) {
        *self.master.lock().unwrap() = level.clamp(0.0, 2.0);
    }
}
