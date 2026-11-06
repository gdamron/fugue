//! Ported from Mutable Instruments' `plaits/dsp/fx/overdrive.h` (MIT,
//! Copyright 2014 Emilie Gillet). See THIRD_PARTY_NOTICES.md.
//!
//! A soft-clipping overdrive whose output gain makes up for its drive, so
//! turning it up changes the colour more than the loudness.

use super::super::filters::{soft_clip, BlockGlide};

#[derive(Default)]
pub(super) struct Overdrive {
    pre_gain: BlockGlide,
    post_gain: BlockGlide,
}

impl Overdrive {
    /// Drives `block` in place, by `drive` from 0 (clean) to 1 (hard).
    pub fn process(&mut self, drive: f32, block: &mut [f32]) {
        let drive_2 = drive * drive;
        let pre_gain_a = drive * 0.5;
        let pre_gain_b = drive_2 * drive_2 * drive * 24.0;
        let pre_gain = pre_gain_a + (pre_gain_b - pre_gain_a) * drive_2;
        let drive_squashed = drive * (2.0 - drive);
        let post_gain = 1.0 / soft_clip(0.33 + drive_squashed * (pre_gain - 0.33));
        self.pre_gain.start(pre_gain, block.len());
        self.post_gain.start(post_gain, block.len());
        for s in block.iter_mut() {
            let pre = self.pre_gain.next() * *s;
            *s = soft_clip(pre) * self.post_gain.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harder_drive_squares_a_sine_up() {
        let crest = |drive: f32| {
            let mut overdrive = Overdrive::default();
            let mut peak = 0.0f32;
            let mut energy = 0.0f32;
            for block in 0..400 {
                let mut x: Vec<f32> = (0..12)
                    .map(|i| (((block * 12 + i) as f32) * 0.01).sin() * 0.5)
                    .collect();
                overdrive.process(drive, &mut x);
                if block >= 200 {
                    for s in &x {
                        peak = peak.max(s.abs());
                        energy += s * s;
                    }
                }
            }
            peak / (energy / 2_400.0).sqrt()
        };
        // A sine's crest factor is √2; a squarer wave's is nearer 1.
        assert!((crest(0.5) - 2f32.sqrt()).abs() < 0.1, "{}", crest(0.5));
        assert!(crest(1.0) < 1.25, "{}", crest(1.0));
    }
}
