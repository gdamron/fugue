//! The voice around the engines: the model crossfade, the trigger, and the
//! low-pass gate.
//!
//! The trigger handling and the gate's settings are ported from Mutable
//! Instruments' `plaits/dsp/voice.cc` (MIT, Copyright 2016 Emilie Gillet),
//! adapted to Fugue's block processing. The crossfade between engines is
//! Fugue's own. See THIRD_PARTY_NOTICES.md.

use super::engine::{semitones_to_ratio, EngineParameters, ENGINE_BLOCK};
use super::engines::{Engines, MODEL_COUNT};
use super::lpg::{LowPassGate, LpgEnvelope, TUNED_RATE};
use crate::dsp::BlockRamp;

/// How long a model change takes to crossfade, in seconds.
const CROSSFADE_SECONDS: f32 = 0.02;

/// The gate's colour: 0 is a dark, filter-like close, 1 a bright, VCA-like
/// one. The middle is the classic plucked sound.
const LPG_COLOUR: f32 = 0.5;

/// How long patching or unpatching `trigger` and `level` takes to fade
/// between the gated and the ungated sound, in seconds.
const BYPASS_SECONDS: f32 = 0.005;

/// A trigger is high above this and low again below [`TRIGGER_LOW`].
const TRIGGER_HIGH: f32 = 0.3;
const TRIGGER_LOW: f32 = 0.1;

/// What drives the low-pass gate this block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Gate {
    /// Neither `trigger` nor `level` is patched: the gate is bypassed.
    Open,
    /// `trigger` is patched alone: each rising edge pings the gate.
    Ping { rising: bool },
    /// `level` is patched: it sets how far the gate is open, 0 to 1.
    Level(f32),
}

/// One block's request to the voice.
pub(super) struct Block {
    pub model: usize,
    /// The pitch, in semitones (69 is A 440).
    pub note: f32,
    pub harmonics: f32,
    pub timbre: f32,
    pub morph: f32,
    pub gate: Gate,
    /// The gate's `decay` mapped to the 0-to-1 scale the gate was tuned on.
    pub decay: f32,
    /// The key and scale chords are built in.
    pub key: u8,
    pub scale: u8,
}

pub(super) struct Voice {
    sample_period: f32,
    /// How far each frame moves a crossfade.
    fade_step: f32,
    /// Coefficient scale for a block of 12 frames at this rate.
    rate_scale: f32,
    engines: Engines,
    /// Each engine's share of the output, 0 to 1. Engines at 0 are not run.
    shares: [f32; MODEL_COUNT],
    envelope: LpgEnvelope,
    gate_audio: LowPassGate,
    gate_aux: LowPassGate,
    /// How far the gate is bypassed, 0 to 1: it fades rather than cuts
    /// when `trigger` or `level` is patched or unpatched.
    bypass: BlockRamp,
    /// How far `bypass` moves per frame.
    bypass_step: f32,
    /// Whether a block has been rendered: the first starts at its own
    /// bypass, with no fade.
    started: bool,
    /// The `decay` the gate coefficients below were computed for, and the
    /// coefficients per block of 12 frames at the tuned rate.
    decay: f32,
    short_decay: f32,
    decay_tail: f32,
    trigger_high: bool,
    out: [f32; ENGINE_BLOCK],
    aux: [f32; ENGINE_BLOCK],
    dry: [f32; ENGINE_BLOCK],
    dry_aux: [f32; ENGINE_BLOCK],
}

impl Voice {
    /// A voice playing `model`, which must be below [`MODEL_COUNT`].
    pub fn new(sample_rate: u32, model: usize) -> Self {
        let rate = sample_rate.max(1) as f32;
        let mut shares = [0.0; MODEL_COUNT];
        shares[model] = 1.0;
        Self {
            sample_period: 1.0 / rate,
            fade_step: 1.0 / (CROSSFADE_SECONDS * rate),
            rate_scale: TUNED_RATE / rate,
            engines: Engines::new(sample_rate),
            shares,
            envelope: LpgEnvelope::new(),
            gate_audio: LowPassGate::new(sample_rate),
            gate_aux: LowPassGate::new(sample_rate),
            bypass: BlockRamp::new(1.0),
            bypass_step: 1.0 / (BYPASS_SECONDS * rate),
            started: false,
            decay: f32::NAN,
            short_decay: 0.0,
            decay_tail: 0.0,
            trigger_high: false,
            out: [0.0; ENGINE_BLOCK],
            aux: [0.0; ENGINE_BLOCK],
            dry: [0.0; ENGINE_BLOCK],
            dry_aux: [0.0; ENGINE_BLOCK],
        }
    }

    /// Starts the voice on `model` (below [`MODEL_COUNT`]) with no
    /// crossfade, as a freshly built voice would.
    pub fn start_on(&mut self, model: usize) {
        self.shares = [0.0; MODEL_COUNT];
        self.shares[model] = 1.0;
    }

    /// Whether `value` is a trigger's rising edge, with hysteresis so a
    /// slow or noisy edge counts once.
    pub fn trigger_edge(&mut self, value: f32) -> bool {
        if !self.trigger_high && value > TRIGGER_HIGH {
            self.trigger_high = true;
            true
        } else {
            if self.trigger_high && value < TRIGGER_LOW {
                self.trigger_high = false;
            }
            false
        }
    }

    /// Renders up to [`ENGINE_BLOCK`] frames into `audio` and `aux`.
    pub fn render(&mut self, block: &Block, audio: &mut [f32], aux: &mut [f32]) {
        let size = audio.len().min(ENGINE_BLOCK);
        let audio = &mut audio[..size];
        let aux = &mut aux[..size];
        audio.fill(0.0);
        aux.fill(0.0);

        let gate = match block.gate {
            Gate::Level(level) => Gate::Level(compress_level(level)),
            gate => gate,
        };
        let accent = match gate {
            Gate::Level(level) => level,
            _ => 0.8,
        };
        let p = EngineParameters {
            note: block.note,
            sample_period: self.sample_period,
            harmonics: block.harmonics,
            timbre: block.timbre,
            morph: block.morph,
            accent,
            key: block.key,
            scale: block.scale,
        };
        self.mix_engines(block.model, &p, audio, aux);
        self.apply_gate(gate, block.decay, &p, audio, aux);
    }

    /// Runs every engine with a share of the output and sums them, moving
    /// each share toward 1 for `model` and 0 for the rest.
    fn mix_engines(
        &mut self,
        model: usize,
        p: &EngineParameters,
        audio: &mut [f32],
        aux: &mut [f32],
    ) {
        let size = audio.len();
        let travel = self.fade_step * size as f32;
        for engine in 0..MODEL_COUNT {
            let from = self.shares[engine];
            let to = if engine == model {
                (from + travel).min(1.0)
            } else {
                (from - travel).max(0.0)
            };
            self.shares[engine] = to;
            if from == 0.0 && to == 0.0 {
                continue;
            }
            let out = &mut self.out[..size];
            let side = &mut self.aux[..size];
            self.engines.render(engine, p, out, side);
            let step = (to - from) / size as f32;
            let mut share = from;
            for i in 0..size {
                share += step;
                // A raised-cosine-like curve: smooth at both ends.
                let weight = share * share * (3.0 - 2.0 * share);
                audio[i] += out[i] * weight;
                aux[i] += side[i] * weight;
            }
        }
    }

    fn apply_gate(
        &mut self,
        gate: Gate,
        decay: f32,
        p: &EngineParameters,
        audio: &mut [f32],
        aux: &mut [f32],
    ) {
        let size = audio.len();
        if decay.to_bits() != self.decay.to_bits() {
            self.decay = decay;
            self.short_decay = 0.05 * semitones_to_ratio(-96.0 * decay);
            self.decay_tail =
                0.005 * semitones_to_ratio(-72.0 * decay + 12.0 * LPG_COLOUR) - self.short_decay;
        }
        // The gate's coefficients are per 12 frames at 48 kHz.
        let scale = size as f32 / ENGINE_BLOCK as f32 * self.rate_scale;
        let (short_decay, decay_tail) = (self.short_decay * scale, self.decay_tail * scale);
        let rise = (0.6 * scale).min(1.0);
        let bypass_target = if gate == Gate::Open { 1.0 } else { 0.0 };
        if !self.started {
            self.started = true;
            self.bypass.snap(bypass_target);
        }
        let from = self.bypass.value();
        let travel = self.bypass_step * size as f32;
        let to = if bypass_target > from {
            (from + travel).min(1.0)
        } else {
            (from - travel).max(0.0)
        };
        match gate {
            // The gate opens with the bypass, so an unpatch fades in at the
            // bypass's pace and a trigger patched later decays from open.
            Gate::Open => {
                self.envelope
                    .process_level(to, short_decay, decay_tail, LPG_COLOUR, rise);
            }
            Gate::Ping { rising } => {
                if rising {
                    self.envelope.trigger();
                }
                // The opening rate follows the pitch, as a fraction of the
                // tuned rate per block of 12.
                let tuned_frequency =
                    p.note_to_frequency(p.note) / (self.sample_period * TUNED_RATE);
                let attack = tuned_frequency * ENGINE_BLOCK as f32 * 2.0 * scale;
                self.envelope
                    .process_ping(attack, short_decay, decay_tail, LPG_COLOUR, rise);
            }
            Gate::Level(level) => {
                self.envelope
                    .process_level(level, short_decay, decay_tail, LPG_COLOUR, rise);
            }
        }
        let (gain, cutoff, bleed) = (
            self.envelope.gain(),
            self.envelope.cutoff(),
            self.envelope.hf_bleed(),
        );
        if from == 1.0 && to == 1.0 {
            // Bypassed: the gate still runs, so it is ready when patched.
            self.dry[..size].copy_from_slice(audio);
            self.dry_aux[..size].copy_from_slice(aux);
            self.gate_audio
                .process(gain, cutoff, bleed, &mut self.dry[..size]);
            self.gate_aux
                .process(gain, cutoff, bleed, &mut self.dry_aux[..size]);
            self.bypass.snap(1.0);
            return;
        }
        self.dry[..size].copy_from_slice(audio);
        self.dry_aux[..size].copy_from_slice(aux);
        self.gate_audio.process(gain, cutoff, bleed, audio);
        self.gate_aux.process(gain, cutoff, bleed, aux);
        if from == 0.0 && to == 0.0 {
            self.bypass.snap(0.0);
            return;
        }
        let steps = self.bypass.steps(to, size);
        for (i, b) in steps.enumerate() {
            audio[i] += (self.dry[i] - audio[i]) * b;
            aux[i] += (self.dry_aux[i] - aux[i]) * b;
        }
    }
}

/// A `level` input bent the way a vactrol responds: quick to open, then
/// leveling off, 0 to 1. A level that is not a number counts as closed.
fn compress_level(level: f32) -> f32 {
    let level = level.clamp(-16.0, 16.0);
    let level = 1.3 * level / (0.3 + level.abs());
    if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(model: usize, gate: Gate) -> Block {
        Block {
            model,
            note: 57.0,
            harmonics: 0.5,
            timbre: 0.5,
            morph: 0.5,
            gate,
            decay: 0.5,
            key: 0,
            scale: 0,
        }
    }

    #[test]
    fn a_level_of_zero_is_silent() {
        let mut voice = Voice::new(48_000, 0);
        let (mut audio, mut aux) = ([0.0; 12], [0.0; 12]);
        for _ in 0..200 {
            voice.render(&block(0, Gate::Level(0.0)), &mut audio, &mut aux);
            assert!(audio.iter().chain(aux.iter()).all(|s| *s == 0.0));
        }
    }

    #[test]
    fn a_ping_sounds_then_fades() {
        let mut voice = Voice::new(48_000, 0);
        let (mut audio, mut aux) = ([0.0; 12], [0.0; 12]);
        voice.render(&block(0, Gate::Ping { rising: true }), &mut audio, &mut aux);
        let mut early = 0.0f32;
        for _ in 0..100 {
            voice.render(
                &block(0, Gate::Ping { rising: false }),
                &mut audio,
                &mut aux,
            );
            early = audio.iter().fold(early, |m, s| m.max(s.abs()));
        }
        for _ in 0..8_000 {
            voice.render(
                &block(0, Gate::Ping { rising: false }),
                &mut audio,
                &mut aux,
            );
        }
        let late = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(early > 0.1, "sounds at {early}");
        assert!(late < 1e-3, "fades to {late}");
    }

    fn peak(voice: &mut Voice, gate: Gate) -> f32 {
        let (mut audio, mut aux) = ([0.0; 12], [0.0; 12]);
        voice.render(&block(0, gate), &mut audio, &mut aux);
        audio.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// The RMS of `blocks` blocks of 12 (eight is 2 ms).
    fn rms(voice: &mut Voice, gate: Gate, blocks: usize) -> f32 {
        let (mut audio, mut aux) = ([0.0; 12], [0.0; 12]);
        let mut sum = 0.0;
        for _ in 0..blocks {
            voice.render(&block(0, gate), &mut audio, &mut aux);
            sum += audio.iter().map(|s| s * s).sum::<f32>();
        }
        (sum / (12 * blocks) as f32).sqrt()
    }

    #[test]
    fn patching_a_trigger_fades_rather_than_cuts() {
        let mut voice = Voice::new(48_000, 0);
        rms(&mut voice, Gate::Open, 400);
        let open = rms(&mut voice, Gate::Open, 8);
        // Patched: the sound decays from where it was, not from silence.
        let ping = Gate::Ping { rising: false };
        let first = rms(&mut voice, ping, 8);
        assert!(first > 0.5 * open, "{first} after {open}");
        rms(&mut voice, ping, 2_000);
        // Unpatched: the ungated sound fades in over a few milliseconds.
        let first = rms(&mut voice, Gate::Open, 8);
        assert!(first < 0.6 * open, "{first} straight after unpatching");
        rms(&mut voice, Gate::Open, 40);
        let back = rms(&mut voice, Gate::Open, 8);
        assert!(back > 0.8 * open, "{back}, open {open}");
    }

    #[test]
    fn a_level_that_is_not_a_number_does_not_stick() {
        let mut voice = Voice::new(48_000, 0);
        peak(&mut voice, Gate::Level(f32::NAN));
        peak(&mut voice, Gate::Level(f32::INFINITY));
        let mut loud = 0.0f32;
        for _ in 0..400 {
            let p = peak(&mut voice, Gate::Level(1.0));
            assert!(p.is_finite());
            loud = loud.max(p);
        }
        assert!(loud > 0.1, "recovers to {loud}");
    }

    #[test]
    fn trigger_edges_need_the_signal_to_fall_back() {
        let mut voice = Voice::new(48_000, 0);
        let edges: Vec<bool> = [0.0, 1.0, 1.0, 0.2, 1.0, 0.0, 1.0]
            .iter()
            .map(|v| voice.trigger_edge(*v))
            .collect();
        assert_eq!(edges, [false, true, false, false, false, false, true]);
    }
}
