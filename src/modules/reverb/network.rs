//! The reverb's signal path: a multichannel diffuser feeding a feedback
//! delay network.
//!
//! Both stages work on eight channels at once. The diffuser (after
//! Signalsmith, "Let's write a reverb", 2021) runs four stages of short
//! per-channel delays, a polarity pattern and an orthonormal Hadamard mix,
//! so one input sample leaves as a dense cloud of echoes: the early
//! reflections. The feedback network (Stautner & Puckette, 1982; Jot &
//! Chaigne, 1991) recirculates that cloud through eight delay lines of
//! unrelated lengths, mixed by a Householder reflection, which is lossless,
//! so each line's own gain alone sets how fast the tail decays.

use crate::dsp::{Damper, DelayLine};

use super::tuning::{blend, Tuning};

/// Channels in the diffuser and lines in the feedback network.
pub(super) const CHANNELS: usize = 8;
/// Diffusion stages, each longer than the last.
pub(super) const STAGES: usize = 4;

/// One sample of every channel.
pub(super) type Frame = [f32; CHANNELS];

/// Below this a recirculating value is flushed to zero, so a long decay
/// never slows the audio thread with subnormal arithmetic.
const SUBNORMAL_FLUSH: f32 = 1.0e-20;

/// Which channels each diffusion stage inverts before mixing. Without the
/// inversions the Hadamard mix of a signal arriving on every channel at once
/// would gather it back onto one channel.
const POLARITY: [Frame; STAGES] = [
    [1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, 1.0],
    [-1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, -1.0],
    [1.0, -1.0, -1.0, -1.0, 1.0, -1.0, 1.0, 1.0],
    [-1.0, -1.0, -1.0, -1.0, 1.0, -1.0, -1.0, 1.0],
];

/// An 8-point Hadamard transform, in place and unscaled: every output hears
/// every input at the same level. Each pass multiplies the frame's energy
/// by [`CHANNELS`]; the diffuser takes that out once, after its last stage
/// (see [`DIFFUSER_SCALE`]), rather than scaling every stage.
#[inline]
fn hadamard(frame: &mut Frame) {
    let mut span = 1;
    while span < CHANNELS {
        for start in (0..CHANNELS).step_by(span * 2) {
            for i in start..start + span {
                let (a, b) = (frame[i], frame[i + span]);
                frame[i] = a + b;
                frame[i + span] = a - b;
            }
        }
        span *= 2;
    }
}

/// Undoes the gain of [`STAGES`] unscaled Hadamard passes: (1/√8)⁴.
const DIFFUSER_SCALE: f32 = 1.0 / 64.0;

/// A Householder reflection, in place: each channel minus 2/N of their sum.
/// Lossless (orthogonal) and dense, and it costs one sum rather than a
/// matrix multiply.
#[inline]
fn householder(frame: &mut Frame) {
    let shared = frame.iter().sum::<f32>() * (2.0 / CHANNELS as f32);
    for value in frame.iter_mut() {
        *value -= shared;
    }
}

/// The weighted sum of a frame's channels.
#[inline]
pub(super) fn tap(frame: &Frame, weights: &Frame) -> f32 {
    frame.iter().zip(weights).map(|(x, w)| x * w).sum()
}

/// Delay lines long enough for every offset any tuning can ask of them.
fn lines(capacities: &[usize; CHANNELS]) -> [DelayLine; CHANNELS] {
    std::array::from_fn(|ch| DelayLine::new(capacities[ch]))
}

/// The diffuser: [`STAGES`] stages of delay, polarity and Hadamard mix.
/// Lossless: what leaves it carries the energy that went in.
pub(super) struct Diffuser {
    stages: [[DelayLine; CHANNELS]; STAGES],
}

impl Diffuser {
    /// A diffuser whose stage `s`, channel `ch` holds `capacity[s][ch]`
    /// samples.
    pub(super) fn new(capacity: &[[usize; CHANNELS]; STAGES]) -> Self {
        Self {
            stages: std::array::from_fn(|stage| lines(&capacity[stage])),
        }
    }

    /// Diffuses `frame` in place, reading under `to` with weight `s` and
    /// under `from` for the rest.
    #[inline]
    pub(super) fn process(&mut self, frame: &mut Frame, from: &Tuning, to: &Tuning, s: f32) {
        for (stage, lines) in self.stages.iter_mut().enumerate() {
            for (ch, line) in lines.iter_mut().enumerate() {
                let new = line.read(to.diffusion[stage][ch]);
                let delayed = blend(s, || line.read(from.diffusion[stage][ch]), new);
                line.write_and_advance(frame[ch]);
                frame[ch] = delayed * POLARITY[stage][ch];
            }
            hadamard(frame);
        }
        for value in frame.iter_mut() {
            *value *= DIFFUSER_SCALE;
        }
    }
}

/// The feedback delay network: [`CHANNELS`] lines, each with its own
/// decay gain and a one-pole lowpass, recirculated through a Householder
/// reflection.
pub(super) struct FeedbackNetwork {
    lines: [DelayLine; CHANNELS],
    dampers: [Damper; CHANNELS],
}

impl FeedbackNetwork {
    /// A network whose line `ch` holds `capacity[ch]` samples.
    pub(super) fn new(capacity: &[usize; CHANNELS]) -> Self {
        Self {
            lines: lines(capacity),
            dampers: std::array::from_fn(|_| Damper::new()),
        }
    }

    /// Feeds `input` into the network and returns what each line puts out
    /// this sample, decayed and damped: the tail before it is tapped for
    /// the outputs.
    #[inline]
    pub(super) fn process(
        &mut self,
        input: &Frame,
        from: &Tuning,
        to: &Tuning,
        s: f32,
        damping: f32,
    ) -> Frame {
        let mut out = [0.0; CHANNELS];
        for (ch, line) in self.lines.iter().enumerate() {
            // The gain goes on before the damper, which is linear, so the
            // two tunings blend before the filter sees them.
            let new = line.read(to.lines[ch]) * to.gains[ch];
            let decayed = blend(s, || line.read(from.lines[ch]) * from.gains[ch], new);
            out[ch] = self.dampers[ch].tick(decayed, damping);
        }
        let mut feedback = out;
        householder(&mut feedback);
        for ch in 0..CHANNELS {
            let value = feedback[ch] + input[ch];
            let value = if value.abs() < SUBNORMAL_FLUSH {
                0.0
            } else {
                value
            };
            self.lines[ch].write_and_advance(value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energy(frame: &Frame) -> f32 {
        frame.iter().map(|x| x * x).sum()
    }

    #[test]
    fn both_mixes_keep_a_frames_energy() {
        let frame: Frame = [0.3, -1.0, 0.25, 0.8, 0.0, -0.4, 0.9, 0.1];
        let mut mixed = frame;
        hadamard(&mut mixed);
        let scaled = energy(&mixed) / CHANNELS as f32;
        assert!((scaled - energy(&frame)).abs() < 1e-5);
        let mut reflected = frame;
        householder(&mut reflected);
        assert!((energy(&reflected) - energy(&frame)).abs() < 1e-5);
    }

    #[test]
    fn the_hadamard_mix_spreads_one_channel_over_all() {
        let mut frame = [0.0; CHANNELS];
        frame[3] = 1.0;
        hadamard(&mut frame);
        let level = (CHANNELS as f32).sqrt().recip();
        assert!(
            frame.iter().all(|x| (x.abs() - level).abs() < 1e-6),
            "{frame:?}"
        );
    }
}
