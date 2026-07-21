//! Voice pool and keymap-zone runtime state for the SampleInstrument module.
//!
//! Everything here runs on the audio thread and is allocation-free: a voice
//! start only clones an `Arc` (an atomic increment) and copies plain values.

use std::sync::Arc;

use crate::modules::sample_loading::SampleData;

use super::controls::{ScaledLoop, ZoneAudio};

/// Upper bound on pool voices; bounds per-block work.
pub const MAX_VOICES: usize = 16;

/// Crossfade applied when a note takes over a sounding voice. The stolen
/// note keeps being read through this window while it fades, so both the
/// amplitude and the slope stay continuous — freezing its last value
/// instead would remove the step but leave an audible slope kink. Short
/// enough not to smear the new note's attack.
pub const DECLICK_SECONDS: f32 = 0.003;

/// A zone as the audio thread sees it: the swappable audio plus the fixed
/// keymap data, with the root's frequency precomputed at build time.
pub(crate) struct ZoneRuntime {
    pub audio: ZoneAudio,
    pub key_low: u8,
    pub key_high: u8,
    pub root: u8,
    pub root_freq: f32,
}

/// Resolves a note to its zone: a zone whose key range contains the note
/// wins (nearest root breaks ties between overlapping ranges); otherwise
/// the zone whose range lies nearest the note.
pub(crate) fn resolve_zone(zones: &[ZoneRuntime], note: u8) -> Option<usize> {
    let note = i32::from(note);
    let mut best: Option<(usize, i32, i32)> = None;
    for (index, zone) in zones.iter().enumerate() {
        let low = i32::from(zone.key_low);
        let high = i32::from(zone.key_high);
        let outside = (low - note).max(note - high).max(0);
        let root_distance = (note - i32::from(zone.root)).abs();
        if best
            .map(|(_, out, dist)| (outside, root_distance) < (out, dist))
            .unwrap_or(true)
        {
            best = Some((index, outside, root_distance));
        }
    }
    best.map(|(index, _, _)| index)
}

/// Rounds a frequency in Hz to the nearest MIDI note, clamped to 0..=127.
pub(crate) fn note_from_freq(freq: f32) -> u8 {
    let note = 69.0 + 12.0 * (freq / 440.0).log2();
    note.round().clamp(0.0, 127.0) as u8
}

/// One pool voice. `active` is the lifetime flag; `held` distinguishes a
/// sustaining note (gate high: the sustain loop wraps) from a releasing one
/// (gate fell: playback exits the loop and fades over the release time).
pub(crate) struct Voice {
    /// The buffer latched at note-on. Latching the `Arc` (not the zone
    /// index) means an `asset.<i>` swap never yanks a sounding voice onto
    /// a different buffer mid-note.
    pub sample: Option<Arc<SampleData>>,
    pub loop_region: Option<ScaledLoop>,
    /// The resolved MIDI note this voice is keyed by.
    pub note: u8,
    /// Zone index, for the per-block zone gain.
    pub zone: usize,
    pub position: f64,
    /// Read-head advance per frame: note frequency over zone root frequency.
    pub ratio: f64,
    pub velocity: f32,
    pub active: bool,
    pub held: bool,
    pub release_gain: f32,
    pub release_step: f32,
    /// Allocation ordinal for steal-oldest.
    pub started: u64,
    /// The note this voice was playing before it was stolen, still being
    /// read so the take-over crossfades rather than cutting. `fade_amp` is
    /// its gain frozen at the steal (velocity * zone gain * release).
    fade_sample: Option<Arc<SampleData>>,
    fade_position: f64,
    fade_ratio: f64,
    fade_amp: f32,
    fade_gain: f32,
    fade_step: f32,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            sample: None,
            loop_region: None,
            note: 0,
            zone: 0,
            position: 0.0,
            ratio: 1.0,
            velocity: 1.0,
            active: false,
            held: false,
            release_gain: 1.0,
            release_step: 0.0,
            started: 0,
            fade_sample: None,
            fade_position: 0.0,
            fade_ratio: 1.0,
            fade_amp: 0.0,
            fade_gain: 0.0,
            fade_step: 0.0,
        }
    }

    /// True while this voice still contributes to the mix — either sounding
    /// or crossfading out the note it was stolen from.
    #[inline]
    pub fn audible(&self) -> bool {
        self.active || self.fade_gain > 0.0
    }

    /// Moves the note currently on this voice into the crossfade slot, to be
    /// read and faded over `step` per frame. Called just before
    /// [`Self::start`] takes the voice for a new note. `amp` is the gain the
    /// old note was being mixed at.
    pub fn begin_steal_fade(&mut self, amp: f32, step: f32) {
        // Only one note can fade at a time. A second steal inside the window
        // drops the older tail, which is already near-silent and quieter
        // than the note replacing it.
        self.fade_sample = self.sample.clone();
        self.fade_position = self.position;
        self.fade_ratio = self.ratio;
        self.fade_amp = amp;
        self.fade_gain = 1.0;
        self.fade_step = step;
    }

    /// Reads and advances the stolen note's crossfade tail for this frame.
    #[inline]
    pub fn fade_out(&mut self) -> (f32, f32) {
        if self.fade_gain <= 0.0 {
            return (0.0, 0.0);
        }
        let Some(sample) = self.fade_sample.as_ref() else {
            self.fade_gain = 0.0;
            return (0.0, 0.0);
        };
        if self.fade_position >= sample.len() as f64 {
            self.fade_gain = 0.0;
            self.fade_sample = None;
            return (0.0, 0.0);
        }

        let (l, r) = sample.sample_at(self.fade_position);
        let amp = self.fade_amp * self.fade_gain;
        self.fade_position += self.fade_ratio;
        self.fade_gain -= self.fade_step;
        if self.fade_gain <= 0.0 {
            self.fade_gain = 0.0;
            // Release the buffer on the audio thread only by dropping a
            // clone; the process-wide cache still owns the allocation.
            self.fade_sample = None;
        }
        (l * amp, r * amp)
    }

    pub fn start(
        &mut self,
        note: u8,
        zone_index: usize,
        zone: &ZoneRuntime,
        ratio: f64,
        velocity: f32,
        started: u64,
    ) {
        self.active = zone.audio.sample.len() > 0;
        self.sample = Some(Arc::clone(&zone.audio.sample));
        self.loop_region = zone.audio.loop_region;
        self.note = note;
        self.zone = zone_index;
        self.position = 0.0;
        self.ratio = ratio;
        self.velocity = velocity.max(0.0);
        self.held = true;
        self.release_gain = 1.0;
        self.release_step = 0.0;
        self.started = started;
    }

    /// Begins the release phase: the sustain loop stops wrapping (playback
    /// runs on past the loop end) and the fade below takes `1/step` frames.
    pub fn release(&mut self, step: f32) {
        self.held = false;
        self.release_step = step;
    }

    /// Reads the current stereo frame, pre-gain. Inside a held sustain
    /// loop's crossfade region this blends toward the equivalent position
    /// before the loop start, so the wrap is continuous.
    #[inline]
    pub fn sample_frame(&self) -> (f32, f32) {
        let Some(sample) = self.sample.as_ref() else {
            return (0.0, 0.0);
        };
        if self.held {
            if let Some(lp) = self.loop_region {
                let fade_start = lp.end - lp.crossfade;
                if lp.crossfade > 0.0 && self.position >= fade_start {
                    let t = ((self.position - fade_start) / lp.crossfade) as f32;
                    let (l1, r1) = sample.sample_at(self.position);
                    let (l2, r2) = sample.sample_at(self.position - (lp.end - lp.start));
                    return (l1 + (l2 - l1) * t, r1 + (r2 - r1) * t);
                }
            }
        }
        sample.sample_at(self.position)
    }

    /// Advances the read head one frame: wraps the sustain loop while held,
    /// steps the release fade while releasing, and retires the voice at the
    /// fade floor or the end of the buffer.
    #[inline]
    pub fn advance(&mut self) {
        let Some(sample) = self.sample.as_ref() else {
            self.active = false;
            return;
        };
        if !self.active {
            return;
        }
        self.position += self.ratio;

        if self.held {
            if let Some(lp) = self.loop_region {
                if self.position >= lp.end {
                    // `%=`-style wrap keeps the fractional phase bounded
                    // without accumulating drift across loop passes.
                    self.position = lp.start + (self.position - lp.start) % (lp.end - lp.start);
                }
            }
        } else {
            self.release_gain -= self.release_step;
            if self.release_gain <= 0.0 {
                self.active = false;
            }
        }

        if self.position >= sample.len() as f64 {
            self.active = false;
        }
    }
}
