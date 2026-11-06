//! The `six_op_fm` engine: two FM voices playing the patch set.
//!
//! The voice handling follows Mutable Instruments'
//! `plaits/dsp/engine2/six_op_engine.cc` (MIT, Copyright 2021 Emilie
//! Gillet): `harmonics` picks the patch, each trigger moves to the other
//! voice so the last note rings on, and with nothing patched to `trigger`
//! the engine drones with `morph` scrubbing through the envelopes. Fugue
//! adds the crossfades between patches and modes, and the soft clip is a
//! generic rational one. See THIRD_PARTY_NOTICES.md.

use super::patches::{PATCHES, PATCH_COUNT};
use super::voice::{scrub_length, FmVoice, Play};
use crate::modules::macro_oscillator::engine::EngineParameters;

/// How far past a patch's band `harmonics` must move before the next patch
/// is chosen, as a fraction of a band.
const HYSTERESIS: f32 = 0.15;

/// How long a drone takes to fade between patches, or out when triggers
/// take over, in seconds.
const DRONE_FADE_SECONDS: f32 = 0.02;

/// The voices' sum is scaled by this before the soft clip.
const OUTPUT_SCALE: f32 = 0.5;

pub(in crate::modules::macro_oscillator) struct SixOpEngine {
    voices: [FmVoice; 2],
    /// The voice playing the newest note (or the drone).
    active: usize,
    patch: usize,
    /// Whether the engine was droning last block.
    droning: bool,
    started: bool,
    /// Each patch's drone scrub range, in seconds.
    scrub: [f32; PATCH_COUNT],
    /// Each voice's pitch: the newest note follows the `frequency` input,
    /// and a note left ringing keeps the pitch it had.
    notes: [f32; 2],
}

impl SixOpEngine {
    pub fn new() -> Self {
        Self {
            voices: [FmVoice::new(), FmVoice::new()],
            active: 0,
            patch: 0,
            droning: false,
            started: false,
            scrub: std::array::from_fn(|i| scrub_length(&PATCHES[i])),
            notes: [60.0; 2],
        }
    }

    /// The patch `harmonics` points at, held near band edges.
    fn choose_patch(&mut self, harmonics: f32) -> usize {
        let position = harmonics.clamp(0.0, 1.0) * PATCH_COUNT as f32;
        let centre = self.patch as f32 + 0.5;
        if (position - centre).abs() > 0.5 + HYSTERESIS || !self.started {
            self.patch = (position as usize).min(PATCH_COUNT - 1);
        }
        self.patch
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let patch = self.choose_patch(p.harmonics_target);
        let mut play = Play {
            note: p.note,
            sample_period: p.sample_period,
            timbre: p.timbre,
            morph: p.morph,
            scrub: None,
        };
        if p.untriggered {
            // Squared, so the middle of the range sits near the bright,
            // loud onset where a struck sound changes fastest.
            let morph = p.morph.clamp(0.0, 1.0);
            play.scrub = Some(morph * morph * self.scrub[patch]);
            self.drone(patch);
        } else {
            self.play(p, patch);
        }
        self.started = true;

        self.notes[self.active] = p.note;
        out.fill(0.0);
        for (voice, note) in self.voices.iter_mut().zip(self.notes) {
            voice.render(&PATCHES, &Play { note, ..play }, out);
        }
        for (o, a) in out.iter_mut().zip(aux.iter_mut()) {
            *o = soft_clip(*o * OUTPUT_SCALE);
            *a = *o;
        }
    }

    /// Keeps the drone on `patch`, crossfading to the other voice when the
    /// patch changes.
    fn drone(&mut self, patch: usize) {
        let [first, second] = &mut self.voices;
        let (active, other) = if self.active == 0 {
            (first, second)
        } else {
            (second, first)
        };
        if !self.started {
            // Fade in from silence rather than starting at full level.
            active.drone(&PATCHES, patch, DRONE_FADE_SECONDS);
            other.fade_out(0.0);
        } else if !self.droning
            || (patch != active.patch() && !active.is_fading() && !other.is_fading())
        {
            // A new patch, or triggers handing over to the drone: fade
            // across to the other voice.
            other.drone(&PATCHES, patch, DRONE_FADE_SECONDS);
            active.fade_out(DRONE_FADE_SECONDS);
            self.active = 1 - self.active;
        }
        self.droning = true;
    }

    /// Plays notes on the triggers, alternating voices.
    fn play(&mut self, p: &EngineParameters, patch: usize) {
        if self.droning {
            // The drone lets go as if its key were released.
            self.voices[self.active].note_off();
            self.droning = false;
        }
        if p.trigger {
            self.active = 1 - self.active;
            self.voices[self.active].note_on(&PATCHES, patch, p.note, p.accent);
        }
        let other = 1 - self.active;
        self.voices[other].note_off();
        if !p.trigger_held {
            self.voices[self.active].note_off();
        }
    }
}

/// A smooth limit at ±1: close to the input below about 0.5, and never
/// past ±1.
#[inline]
fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}
