//! Ported from Mutable Instruments' `rings/dsp/resonator.cc` (MIT, Copyright
//! 2015 Emilie Gillet), with the band-pass from `stmlib/dsp/filter.h` (MIT,
//! Copyright 2014 Emilie Gillet), adapted to Fugue's block processing: the
//! modes run eight at a time, their coefficients come from cheap per-block
//! math, and modes a strike no longer feeds ring on instead of stopping.
//! See THIRD_PARTY_NOTICES.md.
//!
//! A modal body: a bank of tuned band-pass filters, one per partial, all fed
//! the same excitation. `structure` stretches the partials from harmonic to
//! bell-like, `brightness` sets how fast the upper ones die away, `damping`
//! how long the whole body rings, and `position` where it was struck.

use crate::dsp::{cosine, exp2, tan_quarter_turn};

use super::t60_seconds;

/// The most partials one voice rings.
pub(crate) const MAX_MODES: usize = 64;
/// Modes run in groups of this many, so the compiler can vectorise them.
const LANES: usize = 8;
const GROUPS: usize = MAX_MODES / LANES;
/// The highest partial, as a fraction of the sample rate.
const MAX_PARTIAL: f32 = 0.49;
/// How much the reference scales the excitation into the bank.
const INPUT_GAIN: f32 = 0.125;
/// A state smaller than this is flushed to zero once per block, so a mode
/// that has died away does not leave the bank running on subnormals.
const FLUSH: f32 = 1.0e-15;

/// The partials' stretch per mode for `structure`: below 0.25 the partials
/// crowd together, from 0.25 to 0.3 they are harmonic, above that they
/// spread out to bells and, near 1, metal.
pub(crate) fn stiffness(structure: f32) -> f32 {
    let g = structure.clamp(0.0, 1.0);
    if g < 0.25 {
        -(0.25 - g) * 0.25
    } else if g < 0.3 {
        0.0
    } else if g < 0.9 {
        // 0.01·10^(2.005·x) − 0.01, with 10^y = 2^(y·log2 10).
        let x = (g - 0.3) / 0.6;
        0.01 * exp2(x * 2.005 * std::f32::consts::LOG2_10) - 0.01
    } else {
        let x = (g - 0.9) / 0.1;
        1.5 - 0.5 * cosine(0.5 * x * x)
    }
}

/// One block's settings for a modal voice.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModalSettings {
    /// The fundamental, as a fraction of the sample rate.
    pub frequency: f32,
    pub structure: f32,
    pub brightness: f32,
    pub damping: f32,
    pub position: f32,
}

/// Eight modes' filter coefficients and states.
#[derive(Clone, Copy, Default)]
struct Group {
    /// `tan(π·f)` for each mode's frequency.
    g: [f32; LANES],
    /// `1/q + g`.
    r_plus_g: [f32; LANES],
    /// `1/(1 + g/q + g²)`.
    h: [f32; LANES],
    first: [f32; LANES],
    second: [f32; LANES],
    /// 1 for a mode the excitation reaches, 0 for one left to ring out.
    feed: [f32; LANES],
    /// The mode's pickup level, gliding across the block.
    amplitude: [f32; LANES],
    amplitude_step: [f32; LANES],
}

/// One voice's bank of modes.
pub(crate) struct ModalVoice {
    sample_rate: f32,
    groups: [Group; GROUPS],
    /// How many modes the excitation feeds (set at each strike).
    resolution: usize,
    /// How many modes run: the fed ones and any still ringing beyond them.
    running: usize,
}

impl ModalVoice {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as f32,
            groups: [Group::default(); GROUPS],
            resolution: MAX_MODES,
            running: 0,
        }
    }

    /// How many modes a strike feeds from now on (made even, at most
    /// [`MAX_MODES`]). Modes beyond it keep ringing until the voice falls
    /// quiet, so a smaller resolution never cuts a tail.
    pub(crate) fn set_resolution(&mut self, modes: usize) {
        self.resolution = (modes - modes % 2).clamp(2, MAX_MODES);
    }

    /// How many modes a strike feeds.
    #[cfg(test)]
    pub(crate) fn resolution(&self) -> usize {
        self.resolution
    }

    /// Silences the voice at once.
    pub(crate) fn reset(&mut self) {
        self.groups = [Group::default(); GROUPS];
        self.running = 0;
    }

    /// Works out every mode's coefficients and pickup levels for a block of
    /// `frames`, returning how many modes the excitation reaches.
    fn configure(&mut self, settings: &ModalSettings, frames: usize) -> usize {
        let structure = settings.structure.clamp(0.0, 1.0);
        let mut stiffness = stiffness(structure);
        let f0 = settings.frequency.clamp(1.0e-5, MAX_PARTIAL);
        // The fundamental's q, so that it falls 60 dB in the stated time:
        // a band-pass of quality Q at f rings for 2.2·Q/f seconds.
        let mut q = t60_seconds(settings.damping) * self.sample_rate / 2.199;
        let mut attenuation = 1.0 - structure;
        attenuation *= attenuation;
        attenuation *= attenuation;
        attenuation *= attenuation;
        let brightness = settings.brightness.clamp(0.0, 1.0) * (1.0 - 0.2 * attenuation);
        let mut q_loss = brightness * (2.0 - brightness) * 0.85 + 0.15;
        let q_loss_damping_rate = structure * (2.0 - structure) * 0.1;

        let mut harmonic = f0;
        let mut stretch = 1.0f32;
        let mut audible = 0;
        let position = settings.position.clamp(0.0, 1.0);
        let step = 1.0 / frames.max(1) as f32;
        self.running = self.running.max(self.resolution);
        for mode in 0..self.running {
            let mut partial = harmonic * stretch;
            if partial >= MAX_PARTIAL {
                partial = MAX_PARTIAL;
            } else if mode < self.resolution {
                audible = mode + 1;
            }
            let g = tan_quarter_turn(std::f32::consts::PI * partial);
            let r = 1.0 / (1.0 + partial * q);
            let group = &mut self.groups[mode / LANES];
            let lane = mode % LANES;
            group.g[lane] = g;
            group.r_plus_g[lane] = r + g;
            group.h[lane] = 1.0 / (1.0 + r * g + g * g);
            // The pickup: mode k heard at (1 + cos 2πk·position)/2.
            let target = 0.5 + 0.5 * cosine(mode as f32 * position);
            group.amplitude_step[lane] = (target - group.amplitude[lane]) * step;

            stretch += stiffness;
            // Shrinking partials must not fold back below zero; growing
            // ones gain a few extra at the top.
            stiffness *= if stiffness < 0.0 { 0.93 } else { 0.98 };
            // Keeps the highest partials from dying too fast.
            q_loss += q_loss_damping_rate * (1.0 - q_loss);
            harmonic += f0;
            q *= q_loss;
        }
        for mode in 0..self.running {
            self.groups[mode / LANES].feed[mode % LANES] = if mode < audible { 1.0 } else { 0.0 };
        }
        audible
    }

    /// Rings the bank for one block: `input` excites it, and the odd- and
    /// even-numbered partials are added into `odd` and `even`. Returns the
    /// largest state, for the caller to tell when the voice has fallen
    /// silent.
    // The lanes index several arrays at once, which is what vectorises.
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn process(
        &mut self,
        settings: &ModalSettings,
        input: &[f32],
        odd: &mut [f32],
        even: &mut [f32],
    ) -> f32 {
        let frames = input.len().min(odd.len()).min(even.len());
        self.configure(settings, frames);
        let groups = self.running.div_ceil(LANES);
        for i in 0..frames {
            let x = input[i] * INPUT_GAIN;
            let mut sum = [0.0f32; LANES];
            for group in &mut self.groups[..groups] {
                for l in 0..LANES {
                    let hp =
                        (x * group.feed[l] - group.r_plus_g[l] * group.first[l] - group.second[l])
                            * group.h[l];
                    let bp = group.g[l] * hp + group.first[l];
                    group.first[l] = group.g[l] * hp + bp;
                    let lp = group.g[l] * bp + group.second[l];
                    group.second[l] = group.g[l] * bp + lp;
                    group.amplitude[l] += group.amplitude_step[l];
                    sum[l] += group.amplitude[l] * bp;
                }
            }
            odd[i] += sum[0] + sum[2] + sum[4] + sum[6];
            even[i] += sum[1] + sum[3] + sum[5] + sum[7];
        }
        let mut largest = 0.0f32;
        for group in &mut self.groups[..groups] {
            for l in 0..LANES {
                if group.first[l].abs() < FLUSH && group.second[l].abs() < FLUSH {
                    group.first[l] = 0.0;
                    group.second[l] = 0.0;
                }
                largest = largest.max(group.first[l].abs()).max(group.second[l].abs());
            }
        }
        largest
    }
}

#[cfg(test)]
#[path = "modal_tests.rs"]
mod tests;
