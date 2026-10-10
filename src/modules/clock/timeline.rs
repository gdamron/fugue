//! The clock's beat timeline: its position, its reset, and what a reload
//! does to it.

use crate::control_request::{first_sample_reaching, Timeline, BEFORE_START};
use crate::ControlValue;

use super::{Clock, RESET_ON_RELOAD_KEY, TYPE_ID};

impl Clock {
    /// Returns the clock to beat 0: its next sample is at position 0, the
    /// first of a gate, at its current tempo and gate length.
    pub(super) fn reset(&mut self) {
        // Beats 0 to the floor of the position have begun (none before
        // beat 0); the float-to-int cast saturates.
        let begun = (self.beat_position().floor() + 1.0) as u64;
        self.beats_before = self.beats_before.saturating_add(begun);
        self.epoch_sample = self.sample_count + 1;
        self.epoch_beats = 0.0;
        self.started = false;
        self.beats = 0.0;
        self.phase = 0.0;
    }

    /// The position at the latest sample output, or [`BEFORE_START`] when
    /// none has been since the clock was built or reset.
    pub(super) fn beat_position(&self) -> f64 {
        if self.started {
            self.beats
        } else {
            BEFORE_START
        }
    }

    /// The `(sample, beats)` anchor the next samples' positions are measured
    /// from: the epoch, or the latest sample when a tempo change is pending
    /// (the next sample re-anchors there, see `update_signal`).
    fn anchor(&self) -> (u64, f64) {
        if self.started && self.bpm() != self.last_bpm {
            (self.sample_count, self.beats)
        } else {
            (self.epoch_sample, self.epoch_beats)
        }
    }
}

impl Timeline for Clock {
    fn position(&self) -> f64 {
        self.beat_position()
    }

    fn position_after(&self, samples: u64) -> f64 {
        // The very expression `update_signal` computes, so the prediction
        // agrees with the output bit for bit.
        let (sample, beats) = self.anchor();
        let elapsed = self
            .sample_count
            .saturating_add(samples)
            .saturating_sub(sample);
        beats + elapsed as f64 / self.samples_per_beat()
    }

    fn samples_until(&self, beat: f64) -> Option<u64> {
        let samples_per_beat = self.samples_per_beat();
        if !(samples_per_beat.is_finite() && samples_per_beat > 0.0) {
            return None;
        }
        let (sample, beats) = self.anchor();
        let estimate =
            ((beat - beats) * samples_per_beat).ceil() + sample as f64 - self.sample_count as f64;
        first_sample_reaching(|samples| self.position_after(samples), beat, estimate)
    }

    fn beats_before(&self) -> u64 {
        self.beats_before
    }
}

/// Whether a reload resets a clock it keeps (config `reset_on_reload`,
/// default false: a reload keeps the beat position, so an edit lands
/// without a jump in the pulse).
pub(super) fn reset_on_reload(config: &serde_json::Value) -> Result<bool, String> {
    match config.get(RESET_ON_RELOAD_KEY.key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(reset)) => Ok(*reset),
        Some(other) => Err(format!(
            "{TYPE_ID} config '{}' must be true or false, not {other}",
            RESET_ON_RELOAD_KEY.key
        )),
    }
}

/// The writes a reload makes to a clock it keeps: a reset when its config
/// asks for one.
pub(super) fn writes_on_reload(config: &serde_json::Value) -> Vec<(&'static str, ControlValue)> {
    match reset_on_reload(config) {
        Ok(true) => vec![("reset", ControlValue::Bool(true))],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
