//! The frequency ratios the `ratio` output mode offers, chosen by `shift`.
//!
//! Our own sets, built the way the reference builds them (sets of four
//! ratios to the first output, slower below the middle, unison at it,
//! faster above), with ratios picked for Fugue. Each faster set is the
//! reciprocal of the slower set mirrored about unison.

use super::ramp::CHANNELS;

/// A ratio `p/q` of the first output's frequency. An output at `p/q`
/// restarts together with the first every `q` of the first's cycles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ratio {
    pub p: u32,
    pub q: u32,
    value: f32,
}

impl Ratio {
    pub(super) const UNISON: Ratio = Ratio::new(1, 1);

    pub(super) const fn new(p: u32, q: u32) -> Self {
        Self {
            p,
            q,
            value: p as f32 / q as f32,
        }
    }

    #[inline]
    pub(super) fn value(self) -> f32 {
        self.value
    }

    const fn inverse(self) -> Self {
        Self::new(self.q, self.p)
    }
}

/// Sets below unison; the same number above, mirrored.
const SLOWER: usize = 8;
/// Every set: the slower ones, unison, the faster ones.
pub(super) const SETS: usize = 2 * SLOWER + 1;

type Set = [(u32, u32); CHANNELS];

/// Rhythmic divisions for envelopes and LFOs: polyrhythms against the first
/// output, from octaves down to close, slowly drifting ratios.
const CONTROL_SLOWER: [Set; SLOWER] = [
    [(1, 1), (1, 2), (1, 4), (1, 8)],
    [(1, 1), (1, 2), (1, 3), (1, 4)],
    [(1, 1), (2, 3), (1, 2), (1, 3)],
    [(1, 1), (3, 4), (1, 2), (1, 4)],
    [(1, 1), (3, 4), (2, 3), (1, 2)],
    [(1, 1), (4, 5), (2, 3), (1, 2)],
    [(1, 1), (4, 5), (3, 4), (2, 3)],
    [(1, 1), (7, 8), (5, 6), (4, 5)],
];

/// Chords for an oscillator: octaves and subharmonics, fifths and fourths,
/// major and minor triads below, then a gentle detune.
const AUDIO_SLOWER: [Set; SLOWER] = [
    [(1, 1), (1, 2), (1, 4), (1, 8)],
    [(1, 1), (1, 2), (1, 3), (1, 4)],
    [(1, 1), (2, 3), (1, 2), (1, 3)],
    [(1, 1), (3, 4), (2, 3), (1, 2)],
    [(1, 1), (4, 5), (2, 3), (1, 2)],
    [(1, 1), (5, 6), (2, 3), (1, 2)],
    [(1, 1), (8, 9), (4, 5), (2, 3)],
    [(1, 1), (199, 200), (99, 100), (197, 200)],
];

/// Set `index` (0 slowest, `SLOWER` unison, `SETS - 1` fastest) for the
/// control or the audio range.
pub(super) fn set(audio: bool, index: usize) -> [Ratio; CHANNELS] {
    let slower = if audio {
        &AUDIO_SLOWER
    } else {
        &CONTROL_SLOWER
    };
    let index = index.min(SETS - 1);
    let mut out = [Ratio::UNISON; CHANNELS];
    if index == SLOWER {
        return out;
    }
    let (row, invert) = if index < SLOWER {
        (index, false)
    } else {
        (SETS - 1 - index, true)
    };
    for (slot, &(p, q)) in out.iter_mut().zip(slower[row].iter()) {
        let ratio = Ratio::new(p, q);
        *slot = if invert { ratio.inverse() } else { ratio };
    }
    out
}

/// Picks a set from a 0..1 value, holding the current set until the value
/// is clearly past the boundary, so a value resting on one doesn't flicker.
#[derive(Clone, Copy, Debug)]
pub(super) struct SetPicker {
    index: usize,
}

impl Default for SetPicker {
    fn default() -> Self {
        Self { index: SLOWER }
    }
}

const HYSTERESIS: f32 = 0.05;

impl SetPicker {
    #[inline]
    pub(super) fn pick(&mut self, value: f32) -> usize {
        let scaled = value * SETS as f32 - 0.5;
        let lean = if scaled > self.index as f32 {
            -HYSTERESIS
        } else {
            HYSTERESIS
        };
        let q = (scaled + lean + 0.5).floor();
        self.index = q.clamp(0.0, (SETS - 1) as f32) as usize;
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ends_and_middle_are_octaves_and_unison() {
        let values = |audio, i| set(audio, i).map(Ratio::value);
        for audio in [false, true] {
            assert_eq!(values(audio, 0), [1.0, 0.5, 0.25, 0.125]);
            assert_eq!(values(audio, SLOWER), [1.0; 4]);
            assert_eq!(values(audio, SETS - 1), [1.0, 2.0, 4.0, 8.0]);
        }
    }

    #[test]
    fn the_picker_spans_every_set_and_holds_near_a_boundary() {
        let mut picker = SetPicker::default();
        assert_eq!(picker.pick(0.0), 0);
        assert_eq!(picker.pick(1.0), SETS - 1);
        assert_eq!(picker.pick(0.5), SLOWER);
        // Just past the boundary to the next set, it holds.
        let boundary = (SLOWER as f32 + 1.0) / SETS as f32;
        assert_eq!(picker.pick(boundary + 0.001), SLOWER);
        assert_eq!(picker.pick(boundary + 0.01), SLOWER + 1);
    }
}
