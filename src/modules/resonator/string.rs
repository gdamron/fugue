//! Ported from Mutable Instruments' `rings/dsp/string.cc` and `string.h`
//! (MIT, Copyright 2015 Emilie Gillet), adapted to Fugue: delay lines sized
//! for the sample rate, the loss calibrated in seconds, the damping filter's
//! delay compensated exactly for its warped phase, the stiffness allpass
//! always running so dispersion can fade in without a click, and no
//! infinite sustain. See THIRD_PARTY_NOTICES.md.
//!
//! A plucked string: a delay line one period long with a loss filter in
//! the loop. With dispersion, an allpass in the loop stretches the upper
//! partials (a stiff, piano-like string), noise in the delay wobbles them,
//! and a curved bridge makes them buzz.

use std::f32::consts::PI;

use crate::dsp::{exp2, tan_quarter_turn, DcBlocker, DelayLine, Rng, Svf};

use super::tuning::t60_seconds;

/// The delay line: long enough for a string at the `frequency` control's
/// lowest 20 Hz, where the reference's 2048 samples at 48 kHz stop at
/// about 23.5 Hz.
const LINE_SECONDS: f32 = 0.0525;
/// The shortest delay a read or the allpass may use.
const MIN_DELAY: f32 = 4.0;

/// The string's loss filter: a three-tap low-pass whose gain at DC is the
/// per-period loss, gliding across each block.
#[derive(Default)]
struct DampingFilter {
    x1: f32,
    x2: f32,
    brightness: f32,
    brightness_step: f32,
    damping: f32,
    damping_step: f32,
}

impl DampingFilter {
    fn configure(&mut self, damping: f32, brightness: f32, frames: usize) {
        let step = 1.0 / frames.max(1) as f32;
        self.damping_step = (damping - self.damping) * step;
        self.brightness_step = (brightness - self.brightness) * step;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let h0 = (1.0 + self.brightness) * 0.5;
        let h1 = (1.0 - self.brightness) * 0.25;
        let y = self.damping * (h0 * self.x1 + h1 * (x + self.x2));
        self.x2 = self.x1;
        self.x1 = x;
        self.brightness += self.brightness_step;
        self.damping += self.damping_step;
        y
    }
}

/// One block's settings for a string.
#[derive(Clone, Copy, Debug)]
pub(super) struct StringSettings {
    /// Below 0 the bridge curves (buzz), above 0 the string stiffens.
    pub dispersion: f32,
    pub brightness: f32,
    pub damping: f32,
    pub position: f32,
}

/// A value spread linearly over a block, starting where the last ended.
#[derive(Clone, Copy, Default)]
struct Glide {
    value: f32,
    step: f32,
}

impl Glide {
    fn start(&mut self, target: f32, frames: usize) {
        self.step = (target - self.value) / frames.max(1) as f32;
    }

    #[inline]
    fn next(&mut self) -> f32 {
        self.value += self.step;
        self.value
    }
}

/// One string.
pub(super) struct KarplusString {
    sample_rate: f32,
    dispersive: bool,
    line: DelayLine,
    /// The stiffness allpass's own delay (one sample when not dispersive).
    stretch: DelayLine,
    delay: Glide,
    position: Glide,
    dispersion: Glide,
    compensation: Glide,
    damping_filter: DampingFilter,
    loop_filter: Svf,
    dc_blocker: DcBlocker,
    rng: Rng,
    dispersion_noise: f32,
    curved_bridge: f32,
    /// The frequency the string is playing, for a glide to a new one.
    frequency: f32,
}

impl KarplusString {
    pub(super) fn new(sample_rate: u32, dispersive: bool, seed: u64) -> Self {
        let rate = sample_rate.max(1) as f32;
        let length = (LINE_SECONDS * rate).ceil() as usize + 4;
        let mut string = Self {
            sample_rate: rate,
            dispersive,
            line: DelayLine::new(length),
            stretch: DelayLine::new(if dispersive { length / 2 } else { 1 }),
            delay: Glide::default(),
            position: Glide::default(),
            dispersion: Glide::default(),
            compensation: Glide::default(),
            damping_filter: DampingFilter::default(),
            loop_filter: Svf::new(sample_rate, rate * 0.25, 0.5),
            dc_blocker: DcBlocker::with_cutoff(sample_rate, 20.0),
            rng: Rng::new(seed),
            dispersion_noise: 0.0,
            curved_bridge: 0.0,
            frequency: 220.0 / rate,
        };
        string.delay.value = 1.0 / string.frequency;
        string
    }

    /// The frequency the string glides to, as a fraction of the sample rate;
    /// `glide` 1 jumps there, smaller values move part of the way each block.
    pub(super) fn glide_to(&mut self, frequency: f32, glide: f32) {
        self.frequency += glide * (frequency - self.frequency);
    }

    /// Silences the string at once.
    pub(super) fn reset(&mut self) {
        clear(&mut self.line);
        clear(&mut self.stretch);
        self.damping_filter = DampingFilter::default();
        self.loop_filter.reset();
        self.dc_blocker.reset();
        self.dispersion_noise = 0.0;
        self.curved_bridge = 0.0;
    }

    /// Plays one block: `input` excites the string, its sound is added into
    /// `out` and its pickup at `position` into `aux`. Returns the largest
    /// sample written into the loop, for telling when it has fallen quiet.
    pub(super) fn process(
        &mut self,
        settings: &StringSettings,
        input: &[f32],
        out: &mut [f32],
        aux: &mut [f32],
    ) -> f32 {
        self.frequency = self.frequency.clamp(1.0e-5, 0.49);
        let frames = input.len().min(out.len()).min(aux.len());
        let longest = (self.line.len() - 4) as f32;
        let delay = (1.0 / self.frequency).clamp(MIN_DELAY, longest);
        let clamped_position = 0.5 - 0.98 * (settings.position.clamp(0.0, 1.0) - 0.5).abs();
        self.delay.start(delay, frames);
        self.position.start(clamped_position, frames);
        let dispersion = if self.dispersive {
            settings.dispersion.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.dispersion.start(dispersion, frames);

        let damping = settings.damping.clamp(0.0, 1.0);
        let brightness_control = settings.brightness.clamp(0.0, 1.0);
        // The loss per period that makes the fundamental fall 60 dB in the
        // stated time.
        let t60 = t60_seconds(damping) * self.sample_rate;
        let damping_coefficient = exp2((-10.0 * delay / t60).max(-127.0 / 12.0));
        let brightness = brightness_control * brightness_control;
        let noise_filter = exp2((brightness_control - 1.0) * 4.0);
        let cutoff_semitones = (24.0 + damping * damping * 48.0 + brightness * 24.0).min(84.0);
        let cutoff = (self.frequency * exp2(cutoff_semitones / 12.0)).min(0.499);
        self.damping_filter
            .configure(damping_coefficient, brightness, frames);
        self.loop_filter.set(cutoff * self.sample_rate, 0.5);
        // The loop filter delays the fundamental by its phase: a critically
        // damped two-pole low-pass shifts it by 2·atan(Ω), Ω the warped
        // ratio of the pitch to the cutoff. Shorten the line to match.
        let omega = tan_quarter_turn(PI * self.frequency) / tan_quarter_turn(PI * cutoff);
        self.compensation.start(1.0 - omega.atan() / PI, frames);

        let mut largest = 0.0f32;
        for i in 0..frames {
            let period = self.delay.next();
            let comb_delay = period * self.position.next();
            // The loss filter's own one sample, then the loop filter's phase.
            let mut delay = period * self.compensation.next() - 1.0;
            let mut s = if self.dispersive {
                self.disperse(&mut delay, noise_filter)
            } else {
                self.line.read_fractional(delay)
            };
            s += input[i];
            s = self.damping_filter.process(s);
            s = self.loop_filter.process(s).lowpass;
            self.line.write_and_advance(s);
            largest = largest.max(s.abs());
            out[i] += s;
            aux[i] += self.line.read_fractional(comb_delay.max(1.0));
        }
        largest
    }

    /// Reads the line through the stiffness allpass, with the dispersion
    /// noise and the bridge's curve.
    #[inline]
    fn disperse(&mut self, delay: &mut f32, noise_filter: f32) -> f32 {
        let noise = self.rng.bipolar() / (0.2 + noise_filter);
        self.dispersion_noise += noise_filter * (noise - self.dispersion_noise);

        let dispersion = self.dispersion.next();
        let stretch_point = if dispersion <= 0.0 {
            0.0
        } else {
            dispersion * (2.0 - dispersion) * 0.475
        };
        let noise_amount = if dispersion > 0.75 {
            let n = 4.0 * (dispersion - 0.75);
            n * n * 0.025
        } else {
            0.0
        };
        let curving = (-dispersion).max(0.0);
        let ap_gain = -0.618 * dispersion / (0.15 + dispersion.abs());
        *delay *= 1.0 + self.dispersion_noise * noise_amount
            - self.curved_bridge * curving * curving * 0.01;

        // The allpass always runs, at least its shortest delay, so that it
        // holds the string's recent past when dispersion fades in. Below
        // that it fades its gain out instead of switching off.
        let raw = *delay * stretch_point;
        // Whole samples, so the main read takes up the fraction and the
        // loop keeps its full length.
        let ap_delay = raw
            .max(MIN_DELAY)
            .min((self.stretch.len() - 1) as f32)
            .floor();
        let ap_gain = ap_gain * (raw / MIN_DELAY).min(1.0);
        let main = (*delay - ap_delay).max(1.0);
        let s = self.line.read_fractional(main);
        let read = self.stretch.read(ap_delay as usize);
        let write = s + ap_gain * read;
        self.stretch.write_and_advance(write);
        let mut s = read - ap_gain * write;

        let s_ac = self.dc_blocker.process(s);
        s += curving * (s_ac - s);
        let value = s.abs() - 0.025;
        let sign = if s > 0.0 { 1.0 } else { -1.5 };
        self.curved_bridge = (value.abs() + value) * sign;
        s
    }
}

/// Zeroes `line` in place.
fn clear(line: &mut DelayLine) {
    for _ in 0..line.len() {
        line.write_and_advance(0.0);
    }
}

#[cfg(test)]
#[path = "string_tests.rs"]
mod tests;
