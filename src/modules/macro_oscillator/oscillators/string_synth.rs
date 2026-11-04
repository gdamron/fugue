//! Ported from Mutable Instruments' `plaits/dsp/oscillator/string_synth_oscillator.h`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::{next_blep_sample, this_blep_sample};

/// Stops in a registration: saw and square at 8', 4' and 2', then saw 1'.
pub(in crate::modules::macro_oscillator) const STOPS: usize = 7;

/// A divide-down organ voice: saws at four octaves from one phase
/// counter, with the squares made from pairs of saws (a square is twice
/// a saw minus the saw an octave up), mixed by a registration of
/// [`STOPS`] levels. Every edge is band-limited.
pub(in crate::modules::macro_oscillator) struct StringSynthOscillator {
    /// Position across eight cycles of the top saw, 0 to 8.
    phase: f32,
    next_sample: f32,
    segment: usize,
    frequency: BlockRamp,
    /// The 8', 4', 2' and 1' saws' gains.
    gains: [BlockRamp; 4],
}

impl StringSynthOscillator {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            next_sample: 0.0,
            segment: 0,
            frequency: BlockRamp::new(0.001),
            gains: [BlockRamp::new(0.0); 4],
        }
    }

    /// Whether every saw's gain has come to rest at zero.
    pub fn is_silent(&self) -> bool {
        self.gains.iter().all(|g| g.value() == 0.0)
    }

    /// Adds a block at `frequency` (cycles per sample of the 8' pitch) to
    /// `out`, with `registration` at `gain`.
    pub fn render(
        &mut self,
        frequency: f32,
        registration: &[f32; STOPS],
        gain: f32,
        out: &mut [f32],
    ) {
        let mut frequency = frequency * 8.0;
        // Very high notes shift down an octave at a time and play from
        // the stops above, rather than alias.
        let mut shift = 0;
        while frequency > 0.5 {
            shift += 2;
            frequency *= 0.5;
        }
        if shift >= 8 {
            return;
        }
        let mut stops = [0.0; STOPS];
        stops[shift..].copy_from_slice(&registration[..STOPS - shift]);
        let targets = [
            (stops[0] + 2.0 * stops[1]) * gain,
            (stops[2] - stops[1] + 2.0 * stops[3]) * gain,
            (stops[4] - stops[3] + 2.0 * stops[5]) * gain,
            (stops[6] - stops[5]) * gain,
        ];

        let size = out.len();
        let mut frequencies = self.frequency.steps(frequency, size);
        let [g8, g4, g2, g1] = &mut self.gains;
        let mut gains = [
            g8.steps(targets[0], size),
            g4.steps(targets[1], size),
            g2.steps(targets[2], size),
            g1.steps(targets[3], size),
        ];
        let mut phase = self.phase;
        let mut next_sample = self.next_sample;
        let mut segment = self.segment;
        for sample in out.iter_mut() {
            let this_sample = next_sample;
            next_sample = 0.0;
            let frequency = frequencies.next().unwrap_or(frequency);
            let [saw_8, saw_4, saw_2, saw_1] = gains.each_mut().map(|g| g.next().unwrap_or(0.0));
            let mut this_sample = this_sample;

            phase += frequency;
            let mut next_segment = phase as usize;
            if next_segment != segment {
                let mut discontinuity = 0.0;
                if next_segment >= 8 {
                    phase -= 8.0;
                    next_segment -= 8;
                    discontinuity -= saw_8;
                }
                if next_segment & 3 == 0 {
                    discontinuity -= saw_4;
                }
                if next_segment & 1 == 0 {
                    discontinuity -= saw_2;
                }
                discontinuity -= saw_1;
                if discontinuity != 0.0 {
                    let t = (phase - next_segment as f32) / frequency;
                    this_sample += this_blep_sample(t) * discontinuity;
                    next_sample += next_blep_sample(t) * discontinuity;
                }
            }
            segment = next_segment;

            next_sample += (phase - 4.0) * saw_8 * 0.125;
            next_sample += (phase - (segment & 4) as f32 - 2.0) * saw_4 * 0.25;
            next_sample += (phase - (segment & 6) as f32 - 1.0) * saw_2 * 0.5;
            next_sample += (phase - (segment & 7) as f32 - 0.5) * saw_1;
            *sample += 2.0 * this_sample;
        }
        self.phase = phase;
        self.next_sample = next_sample;
        self.segment = segment;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 8' saw alone plays at the given pitch, as a saw (falling
    /// harmonics, every one present).
    #[test]
    fn the_eight_foot_saw_plays_at_the_pitch() {
        let mut osc = StringSynthOscillator::new();
        let registration = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let frequency = 100.0 / 48_000.0;
        let mut x = vec![0.0; 48_000];
        for block in x.chunks_mut(12) {
            osc.render(frequency, &registration, 1.0, block);
        }
        let x = &x[4_800..];
        let magnitude = |k: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in x.iter().enumerate() {
                let a = std::f64::consts::TAU * f64::from(k) * 100.0 * i as f64 / 48_000.0;
                re += f64::from(v) * a.cos();
                im += f64::from(v) * a.sin();
            }
            (re * re + im * im).sqrt() / x.len() as f64
        };
        let (h1, h2, h3) = (magnitude(1.0), magnitude(2.0), magnitude(3.0));
        assert!(h1 > 0.2 && h2 > 0.4 * h1 && h2 < 0.6 * h1, "{h1} {h2}");
        assert!(h3 > 0.25 * h1 && h3 < 0.42 * h1, "{h3}");
        assert!(magnitude(1.5) < 0.01 * h1);
    }
}
