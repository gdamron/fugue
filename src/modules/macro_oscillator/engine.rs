//! What every engine is given each block, and the small helpers they share.
//!
//! The polyBLEP residuals are ported from Mutable Instruments'
//! `stmlib/dsp/polyblep.h` (MIT, Copyright 2017 Emilie Gillet). See
//! THIRD_PARTY_NOTICES.md.

/// The most frames an engine renders at once. Engines read their
/// parameters once per block of this size and interpolate across it.
pub(super) const ENGINE_BLOCK: usize = 12;

/// The highest oscillator frequency, as a fraction of the sample rate.
pub(super) const MAX_FREQUENCY: f32 = 0.25;

/// One block's parameters, as the voice hands them to an engine.
#[derive(Clone, Copy, Debug)]
pub(super) struct EngineParameters {
    /// The pitch, in semitones (MIDI note numbers: 69 is A 440).
    pub note: f32,
    /// 1 / sample rate, so engines can turn a pitch into a phase step.
    pub sample_period: f32,
    pub harmonics: f32,
    /// `harmonics` as last written, without the glide: for an engine that
    /// picks from a set by it, so a write makes one choice rather than
    /// passing through every one between.
    pub harmonics_target: f32,
    pub timbre: f32,
    pub morph: f32,
    /// How hard the note was played, 0 to 1.
    #[allow(dead_code)]
    pub accent: f32,
    /// Whether a trigger rose during the block.
    pub trigger: bool,
    /// Whether `trigger` is unpatched: an engine with its own excitation
    /// then excites itself continuously instead of waiting for a strike.
    #[allow(dead_code)]
    pub untriggered: bool,
    /// The key (0 is C) and the position in `music::scales::SCALE_NAMES`
    /// that chords are built in.
    pub key: u8,
    pub scale: u8,
}

impl EngineParameters {
    /// The phase step per sample of `note`, in cycles (A 440 at note 69).
    #[inline]
    pub fn note_to_frequency(&self, note: f32) -> f32 {
        let note = note.clamp(-128.0, 136.0);
        440.0 * semitones_to_ratio(note - 69.0) * self.sample_period
    }
}

/// A frequency ratio for an interval in semitones.
#[inline]
pub(super) fn semitones_to_ratio(semitones: f32) -> f32 {
    (semitones * (1.0 / 12.0)).exp2()
}

/// The 2-point polyBLEP residual for the sample before a step at
/// fraction `t` of a sample.
#[inline]
pub(super) fn this_blep_sample(t: f32) -> f32 {
    0.5 * t * t
}

/// The 2-point polyBLEP residual for the sample after the step.
#[inline]
pub(super) fn next_blep_sample(t: f32) -> f32 {
    let t = 1.0 - t;
    -0.5 * t * t
}

/// The integrated residual (for a change of slope) after the corner.
#[inline]
pub(super) fn next_integrated_blep_sample(t: f32) -> f32 {
    let t1 = 0.5 * t;
    let t2 = t1 * t1;
    let t4 = t2 * t2;
    0.1875 - t1 + 1.5 * t2 - t4
}

/// The integrated residual before the corner.
#[inline]
pub(super) fn this_integrated_blep_sample(t: f32) -> f32 {
    next_integrated_blep_sample(1.0 - t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_69_is_a_440() {
        let p = EngineParameters {
            note: 69.0,
            sample_period: 1.0 / 48_000.0,
            harmonics: 0.5,
            harmonics_target: 0.5,
            timbre: 0.5,
            morph: 0.5,
            accent: 0.8,
            trigger: false,
            untriggered: true,
            key: 0,
            scale: 0,
        };
        assert!((p.note_to_frequency(69.0) * 48_000.0 - 440.0).abs() < 1e-3);
        assert!((p.note_to_frequency(81.0) * 48_000.0 - 880.0).abs() < 1e-2);
    }

    #[test]
    fn blep_residuals_join_at_the_step() {
        // A step just after a sample (t near 1) is all in the sample
        // before; one just before the next (t near 0) is all after.
        assert_eq!(this_blep_sample(0.0), 0.0);
        assert_eq!(next_blep_sample(0.0), -0.5);
        assert_eq!(this_blep_sample(1.0), 0.5);
        assert_eq!(next_blep_sample(1.0), 0.0);
        // The integrated residual is continuous across the corner.
        assert!((this_integrated_blep_sample(1.0) - next_integrated_blep_sample(0.0)).abs() < 1e-6);
    }
}
