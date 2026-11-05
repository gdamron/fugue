//! Ported from Mutable Instruments' `rings/dsp/part.cc` (voice allocation,
//! the excitation filter, the internal exciters, and how each model renders
//! a voice; MIT, Copyright 2015 Emilie Gillet), adapted to Fugue: every
//! model keeps its own voices, so a change of model or polyphony never
//! re-initialises one that is ringing. See THIRD_PARTY_NOTICES.md.
//!
//! The resonator has up to four voices. Each strum moves to the next
//! voice, which takes the current pitch; the others keep theirs and ring
//! on. A voice's mode count, string count and stereo placement are set
//! when it is struck.

use crate::dsp::{cosine, exp2, log2, DcBlocker, LinearRamp, Svf};

use super::modal::{ModalSettings, ModalVoice, MAX_MODES};
use super::plucker::Plucker;
use super::string::{KarplusString, StringSettings};
use super::sympathetic::{string_notes, MAX_STRINGS};
use super::ENGINE_BLOCK;

/// The most voices the resonator plays at once.
pub(super) const VOICES: usize = 4;

/// The models, in `model` control order.
pub(super) const MODELS: &[&str] = &["modal", "sympathetic_strings", "string"];
const MODAL: usize = 0;
const SYMPATHETIC: usize = 1;
const STRING: usize = 2;
/// Each model's output gain, from the reference.
const MODEL_GAINS: [f32; 3] = [1.4, 1.0, 1.4];

/// The sympathetic strings' slow pickup wobble, per string, in hertz.
const LFO_HZ: [f32; MAX_STRINGS] = [0.5, 0.4, 0.35, 0.23, 0.211, 0.2, 0.171, 0.15];

/// Excitation below this does not wake a sleeping voice.
const WAKE: f32 = 1.0e-7;
/// A voice whose loop has stayed below this for [`SLEEP_SECONDS`] is
/// silenced and stops costing anything.
const QUIET: f32 = 1.0e-6;
const SLEEP_SECONDS: f32 = 0.1;
/// How long a voice takes to move between stereo placements.
const SPLIT_SECONDS: f32 = 0.02;

/// One engine block's control values.
#[derive(Clone, Copy, Debug)]
pub(super) struct Patch {
    pub structure: f32,
    pub brightness: f32,
    pub damping: f32,
    pub position: f32,
}

/// One engine block's playing state.
#[derive(Clone, Copy, Debug)]
pub(super) struct Performance {
    pub model: usize,
    pub polyphony: usize,
    /// The pitch, as a fraction of the sample rate.
    pub frequency: f32,
    pub strum: bool,
    /// Whether the internal exciter plays (nothing is patched into `audio`).
    pub internal: bool,
}

/// Whether one model's voice is ringing, and for how long it has been quiet.
#[derive(Clone, Copy, Default)]
struct Wake {
    awake: bool,
    quiet: usize,
}

type Block = [f32; ENGINE_BLOCK];

/// What a voice that is not being struck hears.
const SILENCE: Block = [0.0; ENGINE_BLOCK];

/// The voices of every model, and how strums are dealt to them.
pub(super) struct Voices {
    sample_rate: f32,
    active: usize,
    frequency: [f32; VOICES],
    excitation_filter: [Svf; VOICES],
    plucker: [Plucker; VOICES],
    input_blocker: [DcBlocker; VOICES],
    modal: [ModalVoice; VOICES],
    string: [KarplusString; VOICES],
    sympathetic: Box<[[KarplusString; MAX_STRINGS]; VOICES]>,
    sympathetic_count: [usize; VOICES],
    lfo_phase: [[f32; MAX_STRINGS]; VOICES],
    wake: [[Wake; 3]; VOICES],
    /// 0 sends a voice's odd and even partials to `odd` and `even`; 1
    /// sends the whole voice to one side.
    split: [LinearRamp; VOICES],
    sleep_frames: usize,
    excitation: Block,
    sympathetic_input: Block,
    out: Block,
    aux: Block,
}

impl Voices {
    pub(super) fn new(sample_rate: u32, polyphony: usize) -> Self {
        let rate = sample_rate.max(1) as f32;
        let split_blocks = (SPLIT_SECONDS * rate / ENGINE_BLOCK as f32)
            .round()
            .max(1.0) as u32;
        let seed = |v: usize, s: usize| 0x5eed_0000 + (v * 16 + s) as u64;
        let mut voices = Self {
            sample_rate: rate,
            active: 0,
            frequency: [220.0 / rate; VOICES],
            excitation_filter: [Svf::new(sample_rate, 10.0, 0.8); VOICES],
            plucker: std::array::from_fn(|v| Plucker::new(sample_rate, seed(v, 15))),
            input_blocker: [DcBlocker::with_cutoff(sample_rate, 10.0); VOICES],
            modal: std::array::from_fn(|_| ModalVoice::new(sample_rate)),
            string: std::array::from_fn(|v| KarplusString::new(sample_rate, true, seed(v, 14))),
            sympathetic: Box::new(std::array::from_fn(|v| {
                std::array::from_fn(|s| KarplusString::new(sample_rate, false, seed(v, s)))
            })),
            sympathetic_count: [MAX_STRINGS; VOICES],
            lfo_phase: [[0.0; MAX_STRINGS]; VOICES],
            wake: [[Wake::default(); 3]; VOICES],
            split: std::array::from_fn(|_| LinearRamp::new(0.0, split_blocks)),
            sleep_frames: (SLEEP_SECONDS * rate) as usize,
            excitation: [0.0; ENGINE_BLOCK],
            sympathetic_input: [0.0; ENGINE_BLOCK],
            out: [0.0; ENGINE_BLOCK],
            aux: [0.0; ENGINE_BLOCK],
        };
        for v in 0..VOICES {
            voices.configure(v, polyphony);
            voices.split[v].settle();
        }
        voices
    }

    /// Voice `v`'s pitch, as a fraction of the sample rate.
    #[cfg(test)]
    pub(super) fn frequency(&self, v: usize) -> f32 {
        self.frequency[v]
    }

    /// Sets voice `v` up for `polyphony`: how many modes and strings it
    /// rings, and where in the stereo field it sits.
    fn configure(&mut self, v: usize, polyphony: usize) {
        let polyphony = polyphony.clamp(1, VOICES);
        self.modal[v].set_resolution(MAX_MODES / polyphony - 4);
        self.sympathetic_count[v] = (2 * VOICES / polyphony).max(2);
        // Above one voice, voices alternate between the outputs; the third
        // of three keeps its partials split across both, so neither side
        // carries two voices to the other's one.
        let shared = polyphony == 1 || (polyphony == 3 && v == 2);
        self.split[v].set_target(if shared { 0.0 } else { 1.0 });
        if self.wake[v].iter().all(|w| !w.awake) {
            // Nothing is ringing to move, so a new note starts in place.
            self.split[v].settle();
        }
    }

    /// Plays one engine block: `input` is the excitation (ignored while the
    /// internal exciter plays), and the voices are mixed into `odd` and
    /// `even`.
    pub(super) fn render(
        &mut self,
        performance: &Performance,
        patch: &Patch,
        input: &Block,
        odd: &mut Block,
        even: &mut Block,
    ) {
        let polyphony = performance.polyphony.clamp(1, VOICES);
        if performance.strum {
            self.active = (self.active + 1) % polyphony;
            self.configure(self.active, polyphony);
        }
        self.frequency[self.active] = performance.frequency.clamp(1.0e-5, 0.49);
        odd.fill(0.0);
        even.fill(0.0);
        for v in 0..VOICES {
            if self.wake[v].iter().all(|w| !w.awake) && v != self.active {
                // A silent voice takes up the current polyphony at once.
                self.configure(v, polyphony);
            }
            self.excite(v, performance, patch, input);
            let fed = self.excitation.iter().fold(0.0f32, |m, x| m.max(x.abs())) > WAKE;
            for (model, &gain) in MODEL_GAINS.iter().enumerate() {
                let feeding = fed && model == performance.model;
                if feeding {
                    self.wake[v][model].awake = true;
                }
                if !self.wake[v][model].awake {
                    continue;
                }
                self.out.fill(0.0);
                self.aux.fill(0.0);
                let level = self.ring(v, model, feeding, performance.internal, patch);
                self.mix(v, gain, odd, even);
                let wake = &mut self.wake[v][model];
                if !feeding && level < QUIET {
                    wake.quiet += ENGINE_BLOCK;
                    if wake.quiet >= self.sleep_frames {
                        *wake = Wake::default();
                        match model {
                            MODAL => self.modal[v].reset(),
                            STRING => self.string[v].reset(),
                            _ => self.sympathetic[v]
                                .iter_mut()
                                .for_each(KarplusString::reset),
                        }
                    }
                } else {
                    wake.quiet = 0;
                }
            }
            self.split[v].next_value();
        }
    }

    /// Fills `self.excitation` with what voice `v` is struck with this
    /// block, through its excitation filter.
    fn excite(&mut self, v: usize, performance: &Performance, patch: &Patch, input: &Block) {
        let active = v == self.active;
        let frequency = self.frequency[v];
        let brightness = patch.brightness.clamp(0.0, 1.0);
        let cutoff = brightness * (2.0 - brightness);
        let range = if performance.internal {
            frequency * exp2((cutoff - 0.5) * 8.0)
        } else {
            0.4 * exp2((cutoff - 1.0) * 9.0)
        };
        let filter_cutoff = if active {
            range
        } else {
            10.0 / self.sample_rate
        }
        .min(0.499);
        let q = if performance.internal { 1.5 } else { 0.8 };
        let strike = performance.internal && active && performance.strum;
        if active && !performance.internal {
            self.excitation.copy_from_slice(input);
        } else {
            self.excitation.fill(0.0);
        }
        let strings = performance.model != MODAL;
        if strings && active {
            let count = if performance.model == SYMPATHETIC {
                self.sympathetic_count[v]
            } else {
                1
            };
            let gain = 1.0 / (2.0 * count as f32).sqrt();
            self.excitation.iter_mut().for_each(|x| *x *= gain);
        }
        if !strings && strike {
            // The modal exciter is a pulse, scaled up as the filter narrows.
            self.excitation[0] += 0.25 * exp2(2.0 * filter_cutoff * filter_cutoff) / filter_cutoff;
        }
        let filter = &mut self.excitation_filter[v];
        filter.set(filter_cutoff * self.sample_rate, q);
        for x in self.excitation.iter_mut() {
            *x = filter.process(*x).lowpass;
        }
        if strings {
            if performance.internal {
                if strike {
                    self.plucker[v].trigger(frequency, filter_cutoff * 8.0, patch.position);
                }
                self.plucker[v].process(&mut self.excitation);
            }
            let blocker = &mut self.input_blocker[v];
            for x in self.excitation.iter_mut() {
                *x = blocker.process(*x);
            }
        }
    }

    /// Rings voice `v` of `model` into `self.out` and `self.aux`, fed the
    /// excitation when `feeding`. Returns how loud its loop is.
    fn ring(
        &mut self,
        v: usize,
        model: usize,
        feeding: bool,
        internal: bool,
        patch: &Patch,
    ) -> f32 {
        let input = if feeding { &self.excitation } else { &SILENCE };
        let frequency = self.frequency[v];
        match model {
            MODAL => {
                let settings = ModalSettings {
                    frequency,
                    structure: patch.structure,
                    brightness: patch.brightness * patch.brightness,
                    damping: patch.damping,
                    position: patch.position,
                };
                self.modal[v].process(&settings, input, &mut self.out, &mut self.aux)
            }
            STRING => {
                let s = patch.structure;
                let dispersion = if s < 0.24 {
                    (s - 0.24) * 4.166
                } else if s > 0.26 {
                    (s - 0.26) * 1.351_35
                } else {
                    0.0
                };
                let settings = StringSettings {
                    dispersion,
                    brightness: patch.brightness,
                    damping: patch.damping,
                    position: patch.position,
                };
                let string = &mut self.string[v];
                string.glide_to(frequency, 1.0);
                string.process(&settings, input, &mut self.out, &mut self.aux)
            }
            _ => self.ring_sympathetic(v, feeding, internal, patch),
        }
    }

    /// Rings voice `v`'s sympathetic strings: the first is struck, the
    /// others (when the internal exciter plays) answer it.
    fn ring_sympathetic(&mut self, v: usize, feeding: bool, internal: bool, patch: &Patch) -> f32 {
        let count = self.sympathetic_count[v];
        let note = 69.0 + 12.0 * log2(self.frequency[v] * self.sample_rate / 440.0);
        let mut notes = [0.0f32; MAX_STRINGS];
        string_notes(note, patch.structure, count, &mut notes);
        let lfo_step = ENGINE_BLOCK as f32 / self.sample_rate;
        let mut level = 0.0f32;
        for s in 0..count {
            let phase = &mut self.lfo_phase[v][s];
            *phase += LFO_HZ[s] * lfo_step;
            *phase -= phase.floor();
            let lfo = 0.5 + 0.5 * cosine(*phase);
            let mut brightness = patch.brightness;
            let mut damping = patch.damping;
            let mut position = patch.position;
            let mut glide = 1.0;
            let answering = s > 0 && internal;
            if answering {
                brightness *= 2.0 - brightness;
                brightness *= 2.0 - brightness;
                damping = 0.7 + patch.damping * 0.27;
                // A third of the reference's wobble: the full depth swings the
                // halo's comb enough that a long tail swells again.
                let amount = (0.5 - (0.5 - patch.position).abs()) * 0.3;
                position = patch.position + lfo * amount;
                glide = exp2((brightness - 1.0) * 3.0);
            }
            let index = s as f32 / count as f32;
            let settings = StringSettings {
                dispersion: 0.0,
                brightness,
                damping: damping + index * (0.95 - damping),
                position,
            };
            let hz = 440.0 * exp2((notes[s] - 69.0) / 12.0);
            let string = &mut self.sympathetic[v][s];
            string.glide_to(hz / self.sample_rate, glide);
            let input = if answering {
                &self.sympathetic_input
            } else if feeding {
                &self.excitation
            } else {
                &SILENCE
            };
            level = level.max(string.process(&settings, input, &mut self.out, &mut self.aux));
            if s == 0 {
                let gain = 0.2 / count as f32;
                for i in 0..ENGINE_BLOCK {
                    self.sympathetic_input[i] = gain * (self.out[i] - self.aux[i]);
                }
            }
        }
        level
    }

    /// Adds voice `v`'s block into the outputs at its stereo placement.
    fn mix(&mut self, v: usize, gain: f32, odd: &mut Block, even: &mut Block) {
        let from = self.split[v].value();
        let to = {
            let mut ramp = self.split[v];
            ramp.next_value()
        };
        let step = (to - from) / ENGINE_BLOCK as f32;
        let right = v & 1 == 1;
        for i in 0..ENGINE_BLOCK {
            let split = from + step * (i + 1) as f32;
            let (o, e) = (self.out[i] * gain, self.aux[i] * gain);
            let whole = split * (o - e);
            odd[i] += (1.0 - split) * o + if right { 0.0 } else { whole };
            even[i] += (1.0 - split) * e + if right { whole } else { 0.0 };
        }
    }
}

#[cfg(test)]
#[path = "voices_tests.rs"]
mod tests;
