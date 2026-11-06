//! The wavetable voice `chords` crossfades each chord note to in the
//! upper half of `morph`.
//!
//! The voice's shape (a scan through a list of waves, high notes
//! attenuated) follows Mutable Instruments'
//! `plaits/dsp/oscillator/wavetable_oscillator.h` (MIT, Copyright 2016
//! Emilie Gillet). See THIRD_PARTY_NOTICES.md. The waves are Fugue's own,
//! from [`wave_shapes`](super::wave_shapes), read from band-limited mip
//! levels.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::MAX_FREQUENCY;

use super::wave_shapes::wave_index;
use super::wavetables::{follow_pitch, level_position, wavetables};

/// The waves the voice scans, as (bank, row, column), soft to bright:
/// dark harmonic spectra, then synced triangles and saws, a vowel, and
/// narrow synced pulses.
const WAVES: [(usize, usize, usize); 15] = [
    (0, 0, 0),
    (0, 4, 2),
    (0, 1, 3),
    (0, 0, 3),
    (0, 4, 5),
    (0, 0, 5),
    (0, 6, 6),
    (2, 3, 4),
    (2, 0, 1),
    (2, 0, 3),
    (2, 5, 6),
    (1, 3, 3),
    (2, 0, 7),
    (2, 6, 7),
    (2, 7, 6),
];

/// The waves' positions among all the tables.
const INDICES: [usize; WAVES.len()] = {
    let mut indices = [0; WAVES.len()];
    let mut i = 0;
    while i < WAVES.len() {
        let (bank, row, column) = WAVES[i];
        indices[i] = wave_index(bank, row, column);
        i += 1;
    }
    indices
};

/// One chord note's wavetable voice.
pub(super) struct ChordWaveVoice {
    phase: f32,
    frequency: BlockRamp,
    level: BlockRamp,
    amplitude: BlockRamp,
    waveform: BlockRamp,
    started: bool,
}

impl ChordWaveVoice {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            frequency: BlockRamp::new(0.0),
            level: BlockRamp::new(0.0),
            amplitude: BlockRamp::new(0.0),
            waveform: BlockRamp::new(0.0),
            started: false,
        }
    }

    /// Whether the voice has come to rest at silence.
    pub fn is_silent(&self) -> bool {
        self.amplitude.value() == 0.0
    }

    /// Adds a block at `frequency` (cycles per sample) and `amplitude` to
    /// `out`, `waveform` (0 to 1) along the waves.
    pub fn render(&mut self, frequency: f32, amplitude: f32, waveform: f32, out: &mut [f32]) {
        let size = out.len();
        let frequency = frequency.clamp(1e-7, MAX_FREQUENCY);
        // Fades the highest notes, which have few harmonics left.
        let amplitude = amplitude * (1.0 - 2.0 * frequency);
        let waveform = waveform.clamp(0.0, 1.0) * (WAVES.len() as f32 - 1.0001);
        // A voice coming back from silence starts where it now is, rather
        // than gliding from where it was.
        if !self.started || self.is_silent() {
            self.started = true;
            self.frequency.snap(frequency);
            self.level.snap(level_position(frequency));
            self.waveform.snap(waveform);
        }
        let level = follow_pitch(&mut self.level, self.frequency.value(), frequency);
        let tables = wavetables();
        let fs = self.frequency.steps(frequency, size);
        let levels = self.level.steps(level, size);
        let amplitudes = self.amplitude.steps(amplitude, size);
        let waveforms = self.waveform.steps(waveform, size);
        for (o, ((f0, level), (amplitude, waveform))) in out
            .iter_mut()
            .zip(fs.zip(levels).zip(amplitudes.zip(waveforms)))
        {
            self.phase += f0;
            self.phase -= self.phase.floor();
            let read = tables.locate(self.phase, level);
            let integral = waveform as usize;
            let fractional = waveform - integral as f32;
            let a = tables.read(INDICES[integral], &read);
            let b = tables.read(INDICES[integral + 1], &read);
            *o += amplitude * (a + (b - a) * fractional);
        }
    }
}
