//! Ported from Mutable Instruments' `plaits/dsp/fx/diffuser.h` (MIT,
//! Copyright 2016 Emilie Gillet), with its delay lengths scaled from
//! 48 kHz to the sample rate and kept as `f32` rather than 12 bits. See
//! THIRD_PARTY_NOTICES.md.
//!
//! Allpasses in series, then a loop of two more through a damped,
//! slowly swaying delay: a short, dense smear that turns sparse clicks
//! into a wash.

use super::super::drums::filters::{BlockGlide, Rate};
use crate::dsp::cosine;

/// The allpasses' coefficient.
const ALLPASS: f32 = 0.625;
/// The lines' lengths at 48 kHz: four allpasses, the loop's two, then the
/// loop's delay.
const LENGTHS: [f32; 7] = [126.0, 180.0, 269.0, 444.0, 1_653.0, 2_010.0, 3_411.0];
/// Where the fourth allpass and the loop's delay are read, at 48 kHz, and
/// how far the sway moves them.
const SWAYED: [(f32, f32); 2] = [(400.0, 43.0), (3_070.0, 340.0)];
/// The sway's rate, in hertz, and how many samples at 48 kHz it holds.
const SWAY_HZ: f32 = 0.3;
const SWAY_HOLD: f32 = 32.0;

/// A delay line read some samples back before each write.
struct Line {
    buffer: Vec<f32>,
    head: usize,
    /// The sample written `length - 1` samples ago is the line's tail.
    tail: usize,
}

impl Line {
    fn new(length: usize) -> Self {
        let length = length.max(2);
        Self {
            // One spare sample, for an interpolated read at the tail.
            buffer: vec![0.0; length + 1],
            head: 0,
            tail: length - 1,
        }
    }

    #[inline]
    fn advance(&mut self) {
        self.head = self.head.checked_sub(1).unwrap_or(self.buffer.len() - 1);
    }

    #[inline]
    fn at(&self, back: usize) -> f32 {
        self.buffer[(self.head + back) % self.buffer.len()]
    }

    #[inline]
    fn read_tail(&self) -> f32 {
        self.at(self.tail)
    }

    #[inline]
    fn read(&self, back: f32) -> f32 {
        let back = back.clamp(1.0, self.tail as f32);
        let whole = back as usize;
        let (a, b) = (self.at(whole), self.at(whole + 1));
        a + (b - a) * (back - whole as f32)
    }

    #[inline]
    fn write(&mut self, x: f32) {
        self.buffer[self.head] = x;
    }
}

/// An allpass over `line`: `x` in, the filtered sample out.
#[inline]
fn allpass(line: &mut Line, delayed: f32, x: f32, coefficient: f32) -> f32 {
    let into = x + coefficient * delayed;
    line.write(into);
    delayed - coefficient * into
}

pub(super) struct Diffuser {
    lines: [Line; 7],
    swayed: [(f32, f32); 2],
    sway_phase: f32,
    sway_step: f32,
    sway_hold: usize,
    sway_left: usize,
    sway: f32,
    damping: f32,
    lp: f32,
    amount: BlockGlide,
    feedback: BlockGlide,
}

impl Diffuser {
    pub fn new(sample_rate: u32) -> Self {
        let rate = Rate::new(sample_rate);
        let scale = rate.at_48k(1.0).recip();
        let sway_hold = ((SWAY_HOLD * scale).round() as usize).max(1);
        Self {
            lines: LENGTHS.map(|l| Line::new((l * scale).round() as usize)),
            swayed: SWAYED.map(|(at, by)| (at * scale, by * scale)),
            sway_phase: 0.0,
            sway_step: SWAY_HZ * sway_hold as f32 / sample_rate.max(1) as f32,
            sway_hold,
            sway_left: 0,
            sway: 0.5,
            damping: rate.coefficient(0.75),
            lp: 0.0,
            amount: BlockGlide::default(),
            feedback: BlockGlide::default(),
        }
    }

    /// Diffuses `amount` (0 to 1) of `in_out`, with the loop feeding back
    /// `feedback` of itself.
    pub fn process(&mut self, amount: f32, feedback: f32, in_out: &mut [f32]) {
        self.amount.start(amount, in_out.len());
        self.feedback.start(feedback, in_out.len());
        for x in in_out.iter_mut() {
            let amount = self.amount.next();
            let feedback = self.feedback.next();
            if self.sway_left == 0 {
                self.sway_left = self.sway_hold;
                self.sway_phase += self.sway_step;
                self.sway_phase -= self.sway_phase.floor();
                self.sway = 0.5 + 0.5 * cosine(self.sway_phase);
            }
            self.sway_left -= 1;
            self.lines.iter_mut().for_each(Line::advance);
            let [ap1, ap2, ap3, ap4, loop_a, loop_b, delay] = &mut self.lines;

            let mut wet = *x;
            for line in [ap1, ap2, ap3] {
                let delayed = line.read_tail();
                wet = allpass(line, delayed, wet, ALLPASS);
            }
            let (at, by) = self.swayed[0];
            let delayed = ap4.read(at + by * self.sway);
            wet = allpass(ap4, delayed, wet, ALLPASS);

            let (at, by) = self.swayed[1];
            wet += feedback * delay.read(at + by * self.sway);
            self.lp += self.damping * (wet - self.lp);
            wet = self.lp;
            let delayed = loop_a.read_tail();
            wet = allpass(loop_a, delayed, wet, -ALLPASS);
            let delayed = loop_b.read_tail();
            wet = allpass(loop_b, delayed, wet, ALLPASS);
            delay.write(wet);

            *x += amount * (2.0 * wet - *x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click comes out spread across tens of milliseconds, and dies
    /// away when the loop feeds back little.
    #[test]
    fn a_click_is_smeared_and_then_fades() {
        let mut diffuser = Diffuser::new(48_000);
        let mut x = vec![0.0; 48_000];
        x[0] = 1.0;
        for block in x.chunks_mut(12) {
            diffuser.process(1.0, 0.5, block);
        }
        let energy = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
        assert!(energy(&x[480..4_800]) > 0.1 * energy(&x[..4_800]));
        let quarters: Vec<f32> = x.chunks(12_000).map(energy).collect();
        assert!(
            quarters.windows(2).all(|w| w[1] < 0.5 * w[0]),
            "{quarters:?}"
        );
        assert!(x.iter().all(|s| s.is_finite() && s.abs() < 2.0));
    }

    #[test]
    fn at_no_amount_it_passes_the_input() {
        let mut diffuser = Diffuser::new(44_100);
        let mut x: Vec<f32> = (0..480).map(|i| (i as f32 * 0.1).sin()).collect();
        let want = x.clone();
        for block in x.chunks_mut(12) {
            diffuser.process(0.0, 0.5, block);
        }
        assert_eq!(x, want);
    }
}
