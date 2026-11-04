//! The oscillators the engines are built from.

mod grainlet;
mod harmonic;
mod slope;
mod string_synth;
mod variable_saw;
mod variable_shape;
mod z;

pub(super) use grainlet::GrainletOscillator;
pub(super) use harmonic::{HarmonicOscillator, HARMONIC_BATCH};
pub(super) use slope::SlopeOscillator;
pub(super) use string_synth::{StringSynthOscillator, STOPS};
pub(super) use variable_saw::VariableSawOscillator;
pub(super) use variable_shape::VariableShapeOscillator;
pub(super) use z::ZOscillator;

#[cfg(test)]
mod tests {
    use super::*;

    /// Rising zero crossings over `blocks` blocks of 12 from `render`.
    fn crossings(blocks: usize, mut render: impl FnMut(&mut [f32])) -> (usize, f32) {
        let (mut count, mut peak, mut last) = (0, 0.0f32, 0.0f32);
        let mut block = [0.0; 12];
        for _ in 0..blocks {
            render(&mut block);
            for &s in &block {
                count += usize::from(last <= 0.0 && s > 0.0);
                peak = peak.max(s.abs());
                last = s;
            }
        }
        (count, peak)
    }

    #[test]
    fn every_shape_plays_at_its_frequency_within_range() {
        // 4,000 blocks of 12 is one second at 48 kHz; 440 Hz is 440 cycles.
        for shape in [0.0, 0.5, 1.0] {
            let mut osc = VariableShapeOscillator::new();
            let (count, peak) = crossings(4_000, |out| {
                osc.render(false, 0.0, 440.0 / 48_000.0, 0.5, shape, out)
            });
            assert!(
                (439..=441).contains(&count),
                "shape {shape}: {count} cycles"
            );
            assert!(peak <= 1.1, "shape {shape} peaks at {peak}");
        }
        for shape in [0.0, 1.0] {
            let mut saw = VariableSawOscillator::new();
            let (count, peak) =
                crossings(4_000, |out| saw.render(440.0 / 48_000.0, 0.5, shape, out));
            assert!((439..=441).contains(&count), "saw {shape}: {count} cycles");
            // The notch overshoots to 7/6, as in the original.
            assert!(peak <= 1.2, "saw {shape} peaks at {peak}");
        }
    }

    #[test]
    fn a_synced_oscillator_repeats_at_the_master_frequency() {
        let mut osc = VariableShapeOscillator::new();
        // A slave a fifth up, reset by a 220 Hz master: one waveform
        // period per master cycle, so its strongest repeat is 220 Hz.
        let mut out = vec![0.0; 48_000];
        for block in out.chunks_mut(12) {
            osc.render(true, 240.0 / 48_000.0, 360.0 / 48_000.0, 0.5, 1.0, block);
        }
        let lag = 200;
        let tail = &out[1_000..40_000];
        let diff: f32 = tail
            .iter()
            .zip(&out[1_000 + lag..])
            .map(|(a, b)| (a - b).abs())
            .sum();
        let energy: f32 = tail.iter().map(|a| a.abs()).sum();
        assert!(diff < 0.2 * energy, "repeats loosely: {diff} vs {energy}");
    }
}
