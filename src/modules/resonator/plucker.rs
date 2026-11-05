//! Ported from Mutable Instruments' `rings/dsp/plucker.h` (MIT, Copyright
//! 2015 Emilie Gillet), adapted to Fugue's sample rates. See
//! THIRD_PARTY_NOTICES.md.

use crate::dsp::{DelayLine, Rng, Svf};

/// The longest comb in the reference, at 48 kHz.
const COMB_AT_48K: f32 = 255.0;

/// The internal exciter for strings: a burst of noise one fraction of a
/// period long, combed at the pluck position and low-passed by brightness.
pub(super) struct Plucker {
    sample_rate: f32,
    comb: DelayLine,
    /// The longest comb delay that fits the line.
    longest: f32,
    filter: Svf,
    rng: Rng,
    remaining: usize,
    period: f32,
    gain: f32,
    /// How long since the burst ended, to know when the comb has emptied.
    quiet: usize,
}

impl Plucker {
    pub(super) fn new(sample_rate: u32, seed: u64) -> Self {
        let rate = sample_rate.max(1) as f32;
        let longest = (COMB_AT_48K * rate / 48_000.0).ceil();
        Self {
            sample_rate: rate,
            comb: DelayLine::new(longest as usize + 2),
            longest,
            filter: Svf::new(sample_rate, rate * 0.25, 1.0),
            rng: Rng::new(seed),
            remaining: 0,
            period: 1.0,
            gain: 0.0,
            quiet: usize::MAX,
        }
    }

    /// Plucks a string of `frequency` (a fraction of the sample rate),
    /// filtered at `cutoff` (the same), picked at `position`.
    pub(super) fn trigger(&mut self, frequency: f32, cutoff: f32, position: f32) {
        let position = position.clamp(0.0, 1.0);
        let mut period = position * 0.9 + 0.05;
        period /= frequency.max(1.0e-5);
        self.remaining = period as usize;
        while period >= self.longest {
            period *= 0.5;
        }
        self.period = period.max(1.0);
        self.gain = (1.0 - position) * 0.8;
        self.filter.set(cutoff.min(0.499) * self.sample_rate, 1.0);
        self.quiet = 0;
    }

    /// Whether the plucker has anything left to play.
    pub(super) fn is_sounding(&self) -> bool {
        self.remaining > 0 || self.quiet < 8 * self.comb.len()
    }

    /// Adds the next `out.len()` frames of the pluck into `out`.
    pub(super) fn process(&mut self, out: &mut [f32]) {
        if !self.is_sounding() {
            return;
        }
        for sample in out.iter_mut() {
            let noise = if self.remaining > 0 {
                self.remaining -= 1;
                self.rng.bipolar()
            } else {
                self.quiet = self.quiet.saturating_add(1);
                0.0
            };
            let combed = noise + self.gain * self.comb.read_fractional(self.period);
            self.comb.write_and_advance(combed);
            *sample += self.filter.process(combed).lowpass;
        }
    }
}
