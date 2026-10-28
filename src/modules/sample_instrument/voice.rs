//! Voice pool and keymap-zone runtime state for the SampleInstrument module.
//!
//! Everything here runs on the audio thread and is allocation-free: a voice
//! start only clones an `Arc` (an atomic increment) and copies plain values.

use std::sync::Arc;

use crate::modules::sample_loading::SampleData;
use crate::payload::Shared;

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
/// keymap data, with the root's frequency precomputed at build time. The
/// audio is swapped in by `asset.N` and the one it replaces retired off
/// the audio thread.
pub(crate) struct ZoneRuntime {
    pub audio: Shared<ZoneAudio>,
    pub key_low: u8,
    pub key_high: u8,
    pub root_note: u8,
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
        let root_distance = (note - i32::from(zone.root_note)).abs();
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
/// (gate fell: the fade runs over the release time, and playback exits the
/// loop once the rest of the sample can hold what is left of the fade).
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
    /// Whether a `note_on` started the voice since the instrument last
    /// rendered: it has sounded nothing yet, so a zone swap at the same
    /// instant reaches it (see [`Self::relatch`]).
    pub fresh: bool,
    /// Whether the sustain loop wraps at its end this pass. Always true
    /// while held; once released, true until a pass whose exit leaves the
    /// tail long enough for the fade to finish in it.
    wraps: bool,
    /// The release fade, in f64 so a long fade at a high rate steps down
    /// to zero in the frames it promises (f32 steps near 1.0 round away a
    /// large share of a tiny slope, and the tail-fit test relies on it).
    release_gain: f64,
    release_step: f64,
    /// Allocation ordinal for steal-oldest.
    pub started: u64,
    /// The note this voice was playing before it was stolen, still being
    /// read so the take-over crossfades rather than cutting. `fade_amp` is
    /// its gain frozen at the steal (velocity * zone gain * release).
    fade_sample: Option<Arc<SampleData>>,
    fade_position: f64,
    fade_ratio: f64,
    /// The stolen note's loop, kept wrapping (and blending) through the
    /// declick when that note was still on its loop.
    fade_loop: Option<ScaledLoop>,
    fade_amp: f32,
    fade_gain: f32,
    fade_step: f32,
    /// The stolen note's zone, and whether that note was `fresh`: a note
    /// stolen at the instant it started has sounded nothing, so a zone swap
    /// at that instant reaches its tail as well (see [`Self::relatch_fade`]).
    pub fade_zone: usize,
    pub fade_fresh: bool,
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
            fresh: false,
            wraps: true,
            release_gain: 1.0,
            release_step: 0.0,
            started: 0,
            fade_sample: None,
            fade_position: 0.0,
            fade_ratio: 1.0,
            fade_loop: None,
            fade_amp: 0.0,
            fade_gain: 0.0,
            fade_step: 0.0,
            fade_zone: 0,
            fade_fresh: false,
        }
    }

    /// True while this voice still contributes to the mix — either sounding
    /// or crossfading out the note it was stolen from.
    #[inline]
    pub fn audible(&self) -> bool {
        self.active || self.fade_gain > 0.0
    }

    /// The release fade's gain on this frame.
    #[inline]
    pub fn release_gain(&self) -> f32 {
        self.release_gain as f32
    }

    /// The loop this voice wraps this pass, if any.
    #[inline]
    fn wrapping(&self) -> Option<ScaledLoop> {
        if self.wraps {
            self.loop_region
        } else {
            None
        }
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
        self.fade_loop = self.wrapping();
        self.fade_amp = amp;
        self.fade_gain = 1.0;
        self.fade_step = step;
        self.fade_zone = self.zone;
        self.fade_fresh = self.fresh;
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

        let (l, r) = read(sample, self.fade_loop, self.fade_position);
        let amp = self.fade_amp * self.fade_gain;
        self.fade_position = wrap(self.fade_loop, self.fade_position + self.fade_ratio);
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
        self.wraps = true;
        self.release_gain = 1.0;
        self.release_step = 0.0;
        self.started = started;
    }

    /// Takes `audio`, its zone's new audio, in place of what it latched at
    /// its start, which it has not sounded yet. Letting go of its clone
    /// frees nothing: the sample cache keeps the buffer (see
    /// [`ZoneAudio::sample`]).
    pub fn relatch(&mut self, audio: &ZoneAudio) {
        self.active = audio.sample.len() > 0;
        self.sample = Some(Arc::clone(&audio.sample));
        self.loop_region = audio.loop_region;
    }

    /// Takes `audio` into the crossfade tail of a note stolen at the
    /// instant it started, as [`Self::relatch`] does for the note itself.
    pub fn relatch_fade(&mut self, audio: &ZoneAudio) {
        self.fade_sample = Some(Arc::clone(&audio.sample));
        self.fade_loop = audio.loop_region;
    }

    /// Begins the release phase: the fade below takes `1/step` frames, and
    /// the sustain loop wraps only while the fade would outlast the rest of
    /// the sample (see [`Self::still_wraps`]).
    pub fn release(&mut self, step: f32) {
        self.held = false;
        self.release_step = f64::from(step);
        self.wraps = self.still_wraps();
    }

    /// Sets the fade of a release begun with no step yet (a note off
    /// waiting for the release in force as it sounds).
    pub fn take_release(&mut self, step: f32) {
        if self.active && !self.held && self.release_step == 0.0 {
            self.release_step = f64::from(step);
            self.wraps = self.still_wraps();
        }
    }

    /// Whether a releasing voice wraps its loop at the end of this pass.
    /// Leaving the loop now plays the rest of the sample, so the voice
    /// leaves only when that holds what remains of the fade; otherwise the
    /// sample would end under a fade still sounding and cut it with a
    /// tick. Wrapping once more never risks that (each pass only shortens
    /// the fade left), so a voice inside the crossfade keeps wrapping
    /// rather than jumping out of the blend, and a fade with no step yet
    /// keeps wrapping until it has one.
    fn still_wraps(&self) -> bool {
        let (Some(sample), Some(lp)) = (self.sample.as_ref(), self.loop_region) else {
            return false;
        };
        if self.release_step <= 0.0 || self.position >= lp.end - lp.crossfade {
            return true;
        }
        // Frames of fade left against frames of sample left, in output
        // frames: gain / step against (len - position) / ratio.
        let rest = (sample.len() as f64 - self.position).max(0.0);
        self.release_gain * self.ratio > rest * self.release_step
    }

    /// Reads the current stereo frame, pre-gain. Inside a wrapping sustain
    /// loop's crossfade region this blends toward the equivalent position
    /// before the loop start, so the wrap is continuous.
    #[inline]
    pub fn sample_frame(&self) -> (f32, f32) {
        let Some(sample) = self.sample.as_ref() else {
            return (0.0, 0.0);
        };
        read(sample, self.wrapping(), self.position)
    }

    /// Advances the read head one frame: wraps the sustain loop while held
    /// (and while releasing, until the tail can hold the fade), steps the
    /// release fade while releasing, and retires the voice at the
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

        let unwrapped = self.position;
        self.position = wrap(self.wrapping(), unwrapped);
        if self.position != unwrapped && !self.held {
            self.wraps = self.still_wraps();
        }
        if !self.held {
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

/// Reads `sample` at `position`. Inside a wrapping loop's crossfade region
/// this blends toward the equivalent position before the loop start, so the
/// wrap is continuous.
#[inline]
fn read(sample: &SampleData, wrapping: Option<ScaledLoop>, position: f64) -> (f32, f32) {
    if let Some(lp) = wrapping {
        let fade_start = lp.end - lp.crossfade;
        if lp.crossfade > 0.0 && position >= fade_start {
            let t = ((position - fade_start) / lp.crossfade) as f32;
            let (l1, r1) = sample.sample_at(position);
            let (l2, r2) = sample.sample_at(position - (lp.end - lp.start));
            return (l1 + (l2 - l1) * t, r1 + (r2 - r1) * t);
        }
    }
    sample.sample_at(position)
}

/// Wraps `position` back into a wrapping loop once it passes the loop end.
/// The `%`-style wrap keeps the fractional phase bounded without
/// accumulating drift across loop passes.
#[inline]
fn wrap(wrapping: Option<ScaledLoop>, position: f64) -> f64 {
    match wrapping {
        Some(lp) if position >= lp.end => lp.start + (position - lp.start) % (lp.end - lp.start),
        _ => position,
    }
}

#[cfg(test)]
mod tests;
