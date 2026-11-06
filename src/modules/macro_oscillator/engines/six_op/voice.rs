//! One FM voice: a patch's operators, envelopes and LFO, played as a note
//! or held as a drone.
//!
//! The way the macros reach the operators follows Mutable Instruments'
//! `plaits/dsp/fm/voice.h` (MIT, Copyright 2021 Emilie Gillet): brightness
//! raises or lowers every modulator's level, and the envelopes can be
//! stretched or read at a point in time. The units, the scaling curves,
//! the crossfades and the choke are Fugue's own. See THIRD_PARTY_NOTICES.md.

use super::algorithms::{is_modulator, Algorithm, ALGORITHMS, OPERATORS};
use super::envelope::{held_level, Envelope, FLOOR_DB};
use super::lfo::Lfo;
use super::operators::Operators;
use super::patch::{Patch, Tune};
use std::f32::consts::{LOG2_10, TAU};

/// How far `timbre` moves the modulators' levels at its ends, in dB.
const BRIGHTNESS_DB: f32 = 12.0;

/// The most modulation index brightness can push a modulator to, in dB re
/// one radian (about 16 radians).
const MAX_INDEX_DB: f32 = 24.0;

/// How fast a drone's envelopes follow the scrub, in dB per second.
const TRACK_DB_PER_SECOND: f32 = 600.0;

/// How fast `timbre`'s brightness reaches the modulators, in dB per
/// second: a modulation index that jumps bends the carriers' phase into a
/// corner, so it slews.
const BRIGHTNESS_DB_PER_SECOND: f32 = 600.0;

/// How long a drone's patch change crossfades, and how long a sounding
/// voice takes to fall silent before it plays a different patch, in
/// seconds.
const CROSSFADE_SECONDS: f32 = 0.02;
const CHOKE_SECONDS: f32 = 0.002;

/// `morph`'s stretch at its ends: envelopes run up to this many times
/// faster or slower than authored.
const MAX_STRETCH_OCTAVES: f32 = 3.0;

/// What a voice is asked to do this block.
#[derive(Clone, Copy, Debug)]
pub(in crate::modules::macro_oscillator) struct Play {
    /// The pitch, in semitones (69 is A 440).
    pub note: f32,
    pub sample_period: f32,
    pub timbre: f32,
    pub morph: f32,
    /// How far into the note a drone sits, in seconds (`None` when the
    /// voice is played by triggers).
    pub scrub: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    patch: usize,
    accent: f32,
    /// Whether it is a drone rather than a note.
    drone: bool,
}

pub(in crate::modules::macro_oscillator) struct FmVoice {
    patch: usize,
    ops: Operators,
    envelopes: [Envelope; OPERATORS],
    lfo: Lfo,
    /// The note the keyboard scaling and the velocity were taken from,
    /// sampled at each note-on (and continuously for a drone).
    scaling_note: f32,
    accent: f32,
    /// Whether the key is down.
    gate: bool,
    /// How long since the note started, for the pitch sweep, in seconds.
    age: f32,
    /// The voice's share of the output, 0 to 1, and where it is heading.
    fade: f32,
    fade_target: f32,
    fade_seconds: f32,
    /// A note waiting for the voice to fall silent first.
    pending: Option<Pending>,
    /// Cached from the patch at note-on: each operator's detune as a ratio,
    /// and the output trim as a gain.
    detune: [f32; OPERATORS],
    gain: f32,
    /// The brightness the modulators are at, in dB, slewing toward
    /// `timbre`'s; `None` when the voice starts from silence and takes it
    /// at once.
    brightness: Option<f32>,
    /// Whether a drone has just started, so its envelopes go straight to
    /// where the scrub points instead of climbing there.
    snap: bool,
    /// How far into the note this patch's drone can be read, in seconds.
    scrub_limit: f32,
}

impl FmVoice {
    pub fn new() -> Self {
        Self {
            patch: 0,
            ops: Operators::new(),
            envelopes: [Envelope::new(); OPERATORS],
            lfo: Lfo::new(),
            scaling_note: 60.0,
            accent: 0.8,
            gate: false,
            age: 0.0,
            fade: 1.0,
            fade_target: 1.0,
            fade_seconds: CROSSFADE_SECONDS,
            pending: None,
            detune: [1.0; OPERATORS],
            gain: 1.0,
            brightness: None,
            snap: false,
            scrub_limit: 0.0,
        }
    }

    pub fn patch(&self) -> usize {
        self.patch
    }

    /// Whether any envelope is still sounding.
    pub fn is_sounding(&self) -> bool {
        self.fade > 0.0 && self.envelopes.iter().any(|e| !e.is_idle())
    }

    /// Whether a crossfade or a choke is under way.
    pub fn is_fading(&self) -> bool {
        self.fade != self.fade_target || self.pending.is_some()
    }

    /// Plays a note of `patch` from the patch set. A voice still sounding a
    /// different patch first falls silent over a couple of milliseconds,
    /// so the change doesn't click.
    pub fn note_on(&mut self, patches: &[Patch], patch: usize, note: f32, accent: f32) {
        self.scaling_note = note;
        if self.is_sounding() && patch != self.patch {
            self.pending = Some(Pending {
                patch,
                accent,
                drone: false,
            });
            self.fade_to(0.0, CHOKE_SECONDS);
            self.gate = true;
            return;
        }
        self.start(patches, patch, accent);
    }

    /// Lets the key go (a note still rising finishes its attack first).
    pub fn note_off(&mut self) {
        self.gate = false;
    }

    /// Holds `patch` as a drone, fading in over `seconds` (0 for at once).
    /// A voice still sounding falls silent first.
    pub fn drone(&mut self, patches: &[Patch], patch: usize, seconds: f32) {
        if seconds > 0.0 && self.is_sounding() {
            self.pending = Some(Pending {
                patch,
                accent: self.accent,
                drone: true,
            });
            self.fade_to(0.0, CHOKE_SECONDS);
            return;
        }
        self.start_drone(patches, patch, seconds);
    }

    fn start_drone(&mut self, patches: &[Patch], patch: usize, seconds: f32) {
        self.load(patches, patch);
        self.pending = None;
        self.envelopes = [Envelope::new(); OPERATORS];
        self.ops.reset_phases();
        self.brightness = None;
        self.snap = true;
        self.scrub_limit = scrub_length(&patches[self.patch]);
        self.lfo.restart();
        self.age = f32::INFINITY;
        self.gate = true;
        self.fade = if seconds > 0.0 { 0.0 } else { 1.0 };
        self.fade_to(1.0, seconds.max(CROSSFADE_SECONDS));
    }

    /// Fades the voice out over `seconds`.
    pub fn fade_out(&mut self, seconds: f32) {
        self.pending = None;
        self.fade_to(0.0, seconds);
    }

    fn fade_to(&mut self, target: f32, seconds: f32) {
        self.fade_target = target;
        self.fade_seconds = seconds;
    }

    fn load(&mut self, patches: &[Patch], patch: usize) {
        self.patch = patch.min(patches.len() - 1);
        let p = &patches[self.patch];
        for (ratio, op) in self.detune.iter_mut().zip(&p.ops) {
            *ratio = (op.detune / 1200.0).exp2();
        }
        self.gain = (p.gain * (LOG2_10 / 20.0)).exp2();
    }

    fn start(&mut self, patches: &[Patch], patch: usize, accent: f32) {
        if !self.is_sounding() {
            self.envelopes = [Envelope::new(); OPERATORS];
            self.ops.reset_phases();
            self.brightness = None;
            self.fade = 1.0;
        }
        self.load(patches, patch);
        self.pending = None;
        // A struck note follows its envelopes rather than a scrub point.
        self.snap = false;
        self.accent = accent;
        self.gate = true;
        self.age = 0.0;
        self.lfo.restart();
        self.envelopes.iter_mut().for_each(Envelope::start);
        // A voice caught partway through a fade comes back up quickly
        // rather than jumping.
        self.fade_to(1.0, CHOKE_SECONDS);
    }

    /// Adds one block to `out`.
    pub fn render(&mut self, patches: &[Patch], play: &Play, out: &mut [f32]) {
        let size = out.len();
        let seconds = size as f32 * play.sample_period;

        // A voice that was already silent stays so; one that falls silent
        // this block still renders it, ramping its last bit down.
        let was_silent = self.fade == 0.0 && self.fade_target == 0.0;

        // The fade, and a choked voice's waiting note.
        let travel = seconds / self.fade_seconds.max(1e-4);
        self.fade = if self.fade < self.fade_target {
            (self.fade + travel).min(self.fade_target)
        } else {
            (self.fade - travel).max(self.fade_target)
        };
        if self.fade == 0.0 {
            match self.pending.take() {
                Some(Pending {
                    patch, drone: true, ..
                }) => {
                    self.start_drone(patches, patch, CROSSFADE_SECONDS);
                }
                Some(Pending { patch, accent, .. }) => {
                    // Silent now: the note starts afresh, fading in as
                    // fast as the choke faded out.
                    self.envelopes = [Envelope::new(); OPERATORS];
                    let gate = self.gate;
                    self.start(patches, patch, accent);
                    self.gate = gate;
                    self.fade = 0.0;
                    self.fade_to(1.0, CHOKE_SECONDS);
                }
                None => {}
            }
        }
        let idle = play.scrub.is_none() && self.envelopes.iter().all(Envelope::is_idle);
        if self.pending.is_none() && (was_silent || idle) {
            return;
        }

        let patch = &patches[self.patch];
        let algorithm: &Algorithm = &ALGORITHMS[usize::from(patch.algorithm - 1)];
        if play.scrub.is_some() {
            self.scaling_note = play.note;
        }
        let lfo = self.lfo.advance(&patch.lfo, seconds);
        self.age += seconds;
        let sweep = if self.age < patch.pitch.time * 4.0 {
            patch.pitch.semitones * (-6.9 * self.age / patch.pitch.time).exp()
        } else {
            0.0
        };
        let pitch = play.note + sweep + lfo * patch.lfo.vibrato * 0.01;
        let f0 = 440.0 * ((pitch.clamp(-128.0, 136.0) - 69.0) / 12.0).exp2() * play.sample_period;

        let octaves = (self.scaling_note - 60.0) / 12.0;
        let stretch = ((play.morph - 0.5) * 2.0 * MAX_STRETCH_OCTAVES).exp2();
        let target = (play.timbre - 0.5) * 2.0 * BRIGHTNESS_DB;
        let brightness = match self.brightness {
            Some(db) => {
                let most = BRIGHTNESS_DB_PER_SECOND * seconds;
                db + (target - db).clamp(-most, most)
            }
            None => target,
        };
        self.brightness = Some(brightness);
        let velocity = (self.accent - 0.8) / 0.8;
        let tremolo = 0.5 * (lfo - 1.0) * patch.lfo.tremolo;

        let mut step = [0.0; OPERATORS];
        let mut level = [0.0; OPERATORS];
        for k in 0..OPERATORS {
            let op = &patch.ops[k];
            step[k] = self.detune[k]
                * match op.tune {
                    Tune::Ratio(r) => f0 * r,
                    Tune::Fixed(hz) => hz * play.sample_period,
                };
            let rate = (op.rate_scale * octaves).exp2();
            let envelope = &mut self.envelopes[k];
            match play.scrub {
                Some(t) => {
                    // Rate scaling moves a high drone further into its
                    // note, but never past where the patch can be read.
                    let at = (t * rate).min(self.scrub_limit);
                    let target = held_level(&op.envelope, at, 1.0);
                    if self.snap {
                        envelope.set(target);
                    } else {
                        envelope.track(target, seconds, TRACK_DB_PER_SECOND);
                    }
                }
                None => {
                    if !self.gate && !envelope.is_attacking() {
                        envelope.release();
                    }
                    envelope.advance(&op.envelope, seconds, stretch / rate);
                }
            }
            if envelope.is_idle() {
                continue;
            }
            let keys = if octaves > 0.0 {
                op.keys_up * octaves
            } else {
                -op.keys_down * octaves
            };
            let mut db = op.level + keys + op.velocity * velocity + op.tremolo * tremolo;
            let modulator = is_modulator(algorithm, k);
            if modulator {
                db = (db + brightness).min(MAX_INDEX_DB.max(op.level));
            }
            db += envelope.db();
            if db <= FLOOR_DB {
                continue;
            }
            let amplitude = (db * (LOG2_10 / 20.0)).exp2();
            level[k] = if modulator {
                amplitude / TAU
            } else {
                amplitude * self.gain * eased(self.fade)
            };
        }
        if play.scrub.is_some() {
            self.snap = false;
        }
        self.ops
            .render(algorithm, &step, &level, patch.feedback, out);
    }
}

/// A fade position shaped to start and end flat, so a crossfade has no
/// corners; a pair crossing over still sums to one.
#[inline]
fn eased(fade: f32) -> f32 {
    fade * fade * (3.0 - 2.0 * fade)
}

/// How far into a note a drone's `morph` can scrub for `patch`: until its
/// loudest heard operator has fallen 30 dB, and no more than 1.5 s.
pub(in crate::modules::macro_oscillator) fn scrub_length(patch: &Patch) -> f32 {
    const LONGEST: f32 = 1.5;
    const STEPS: usize = 150;
    let algorithm = &ALGORITHMS[usize::from(patch.algorithm - 1)];
    let loudest = |t: f32| {
        (0..OPERATORS)
            .filter(|&k| algorithm.is_carrier(k))
            .map(|k| patch.ops[k].level + held_level(&patch.ops[k].envelope, t, 1.0))
            .fold(FLOOR_DB, f32::max)
    };
    let start = loudest(0.0);
    (1..=STEPS)
        .map(|i| LONGEST * i as f32 / STEPS as f32)
        .find(|&t| loudest(t) < start - 30.0)
        .unwrap_or(LONGEST)
}
