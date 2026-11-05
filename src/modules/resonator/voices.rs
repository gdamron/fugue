//! Ported from Mutable Instruments' `rings/dsp/part.cc` (voice allocation,
//! the excitation filter, the internal exciters, and how each model renders
//! a voice; MIT, Copyright 2015 Emilie Gillet), adapted to Fugue: every
//! model keeps its own voices, so a change of model or polyphony never
//! re-initialises one that is ringing. See THIRD_PARTY_NOTICES.md.
//!
//! The resonator has up to four voices. Each strum moves to the next
//! voice, which takes the current pitch; the others keep theirs and ring
//! on. A voice's mode count and stereo placement are set
//! when it is struck.

use crate::dsp::{exp2, LinearRamp, Svf};

use super::modal::{ModalSettings, ModalVoice, MAX_MODES};
use super::ENGINE_BLOCK;

/// The most voices the resonator plays at once.
pub(super) const VOICES: usize = 4;

/// The models, in `model` control order.
pub(super) const MODELS: &[&str] = &["modal"];
/// Each model's output gain, from the reference.
const MODEL_GAINS: [f32; 1] = [1.4];

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
    modal: [ModalVoice; VOICES],
    wake: [[Wake; MODEL_GAINS.len()]; VOICES],
    /// 0 sends a voice's odd and even partials to `odd` and `even`; 1
    /// sends the whole voice to one side.
    split: [LinearRamp; VOICES],
    sleep_frames: usize,
    excitation: Block,
    out: Block,
    aux: Block,
}

impl Voices {
    pub(super) fn new(sample_rate: u32, polyphony: usize) -> Self {
        let rate = sample_rate.max(1) as f32;
        let split_blocks = (SPLIT_SECONDS * rate / ENGINE_BLOCK as f32)
            .round()
            .max(1.0) as u32;
        let mut voices = Self {
            sample_rate: rate,
            active: 0,
            frequency: [220.0 / rate; VOICES],
            excitation_filter: [Svf::new(sample_rate, 10.0, 0.8); VOICES],
            modal: std::array::from_fn(|_| ModalVoice::new(sample_rate)),
            wake: [[Wake::default(); MODEL_GAINS.len()]; VOICES],
            split: std::array::from_fn(|_| LinearRamp::new(0.0, split_blocks)),
            sleep_frames: (SLEEP_SECONDS * rate) as usize,
            excitation: [0.0; ENGINE_BLOCK],
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

    /// Sets voice `v` up for `polyphony`: how many modes it rings, and
    /// where in the stereo field it sits.
    fn configure(&mut self, v: usize, polyphony: usize) {
        let polyphony = polyphony.clamp(1, VOICES);
        self.modal[v].set_resolution(MAX_MODES / polyphony - 4);
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
                let level = self.ring(v, feeding, patch);
                self.mix(v, gain, odd, even);
                let wake = &mut self.wake[v][model];
                if !feeding && level < QUIET {
                    wake.quiet += ENGINE_BLOCK;
                    if wake.quiet >= self.sleep_frames {
                        *wake = Wake::default();
                        self.modal[v].reset();
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
        if strike {
            // The modal exciter is a pulse, scaled up as the filter narrows.
            self.excitation[0] += 0.25 * exp2(2.0 * filter_cutoff * filter_cutoff) / filter_cutoff;
        }
        let filter = &mut self.excitation_filter[v];
        filter.set(filter_cutoff * self.sample_rate, q);
        for x in self.excitation.iter_mut() {
            *x = filter.process(*x).lowpass;
        }
    }

    /// Rings voice `v` into `self.out` and `self.aux`, fed the excitation
    /// when `feeding`. Returns how loud it is.
    fn ring(&mut self, v: usize, feeding: bool, patch: &Patch) -> f32 {
        let input = if feeding { &self.excitation } else { &SILENCE };
        let settings = ModalSettings {
            frequency: self.frequency[v],
            structure: patch.structure,
            brightness: patch.brightness * patch.brightness,
            damping: patch.damping,
            position: patch.position,
        };
        self.modal[v].process(&settings, input, &mut self.out, &mut self.aux)
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
