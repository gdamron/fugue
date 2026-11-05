//! Ported from Mutable Instruments' `rings/dsp/part.cc`
//! (`ComputeSympatheticStringsNotes`, MIT, Copyright 2015 Emilie Gillet).
//! See THIRD_PARTY_NOTICES.md.
//!
//! Where a voice's sympathetic strings are tuned: `structure` slides the
//! second and later strings through a ladder of intervals around the
//! played note (from two octaves down, through fifths and octaves, to two
//! octaves up), and the last few strings double earlier ones slightly
//! detuned, so the halo beats gently.

/// The most strings one voice can have.
pub(super) const MAX_STRINGS: usize = 8;

/// A perfect fifth, in semitones.
const FIFTH: f32 = 7.019_55;
/// How far the doubling strings are detuned, in semitones.
const DETUNINGS: [f32; 4] = [0.013, 0.011, 0.007, 0.017];

/// Pushes `x` (0 to 1) towards 0, ½ and 1, so a string spends most of a
/// sweep on an interval and passes quickly between them.
fn squash(x: f32) -> f32 {
    if x < 0.5 {
        0.5 * (2.0 * x).powi(16)
    } else {
        1.0 - 0.5 * (2.0 - 2.0 * x).powi(16)
    }
}

/// The notes (MIDI note numbers) of `count` strings for a voice playing
/// `note`, with `structure` choosing the intervals. String 0 is the
/// played note.
pub(super) fn string_notes(note: f32, structure: f32, count: usize, notes: &mut [f32]) {
    let count = count.min(MAX_STRINGS).min(notes.len());
    // The ladder's lowest rung stands for the reference's fixed tonic.
    let ladder = [
        note - 24.0,
        note - 12.0,
        note - FIFTH,
        note,
        note + FIFTH,
        note + 12.0,
        note + 12.0 + FIFTH,
        note + 24.0,
        note + 24.0,
    ];
    let doubled = (count.saturating_sub(1)) / 2;
    let first_doubled = count - doubled;
    let mut parameter = structure.clamp(0.0, 1.0);
    for i in 0..first_doubled {
        let rung = if i == 0 {
            3.0
        } else {
            let rung = parameter * 7.0;
            parameter += (1.0 - parameter) * 0.2;
            rung
        };
        let whole = (rung.floor() as usize).min(ladder.len() - 2);
        let fraction = squash(rung - whole as f32);
        let (a, b) = (ladder[whole], ladder[whole + 1]);
        notes[i] = a + (b - a) * fraction;
        if i + first_doubled < count {
            notes[i + first_doubled] = notes[i] + DETUNINGS[i & 3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_string_plays_the_note_and_doublings_beat() {
        let mut notes = [0.0; MAX_STRINGS];
        string_notes(60.0, 0.4, 8, &mut notes);
        assert_eq!(notes[0], 60.0);
        // Eight strings: four tuned, the last three doubling 0..3, detuned.
        for i in 0..3 {
            let beat = notes[i + 5] - notes[i];
            assert!((beat - DETUNINGS[i]).abs() < 1e-4, "string {}", i + 5);
        }
        assert!(notes.iter().all(|n| (36.0..=84.1).contains(n)), "{notes:?}");
    }

    #[test]
    fn two_strings_are_the_note_and_one_interval() {
        let mut notes = [0.0; 2];
        string_notes(60.0, 1.0, 2, &mut notes);
        assert_eq!(notes, [60.0, 84.0]);
    }
}
