//! Named scales over a key: which pitch classes a piece's notes may take.
//!
//! A scale is a set of semitone degrees above its tonic; a key names the
//! tonic's pitch class. Together they make a 12-bit pitch-class mask (bit 0
//! is C), which [`nearest_in`] uses to snap a pitch into the key.

/// The twelve keys by sharp name, in pitch-class order from C.
pub const KEY_NAMES: &[&str] = &[
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Flat spellings of the black-key keys, with the key each means.
pub const KEY_SPELLINGS: &[(&str, &str)] = &[
    ("Db", "C#"),
    ("Eb", "D#"),
    ("Gb", "F#"),
    ("Ab", "G#"),
    ("Bb", "A#"),
];

/// The named scales, in [`scale_degrees`] order.
pub const SCALE_NAMES: &[&str] = &[
    "major",
    "minor",
    "harmonic_minor",
    "melodic_minor",
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "locrian",
    "major_pentatonic",
    "minor_pentatonic",
    "blues",
    "whole_tone",
    "chromatic",
];

const SCALE_DEGREES: [&[u8]; 14] = [
    &[0, 2, 4, 5, 7, 9, 11],
    &[0, 2, 3, 5, 7, 8, 10],
    &[0, 2, 3, 5, 7, 8, 11],
    // Ascending form.
    &[0, 2, 3, 5, 7, 9, 11],
    &[0, 2, 3, 5, 7, 9, 10],
    &[0, 1, 3, 5, 7, 8, 10],
    &[0, 2, 4, 6, 7, 9, 11],
    &[0, 2, 4, 5, 7, 9, 10],
    &[0, 1, 3, 5, 6, 8, 10],
    &[0, 2, 4, 7, 9],
    &[0, 3, 5, 7, 10],
    &[0, 3, 5, 6, 7, 10],
    &[0, 2, 4, 6, 8, 10],
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
];

/// Intervals above the tonic from most to least stable: the order a scale
/// gives up its degrees as it narrows toward its frame (tonic, fifth,
/// thirds, then the rest).
const STABILITY: [u8; 12] = [0, 7, 4, 3, 9, 8, 5, 2, 10, 11, 6, 1];

/// The semitone degrees of the scale at `index` in [`SCALE_NAMES`] (major
/// past the end).
pub fn scale_degrees(index: usize) -> &'static [u8] {
    SCALE_DEGREES
        .get(index)
        .copied()
        .unwrap_or(SCALE_DEGREES[0])
}

/// The `count` most stable of `degrees` (at least one, the most stable
/// present), as a mask of intervals above the tonic.
pub fn strongest_degrees(degrees: &[u8], count: usize) -> u16 {
    let scale = degrees
        .iter()
        .fold(0u16, |mask, &degree| mask | 1 << (degree % 12));
    let mut kept = 0u16;
    let mut taken = 0;
    for interval in STABILITY {
        if taken >= count.max(1) {
            break;
        }
        if scale & 1 << interval != 0 {
            kept |= 1 << interval;
            taken += 1;
        }
    }
    kept
}

/// A mask of intervals above the tonic moved to `key` (0 = C): the pitch
/// classes it allows.
pub fn in_key(intervals: u16, key: u8) -> u16 {
    let key = u32::from(key % 12);
    let wide = u32::from(intervals) << key;
    ((wide | wide >> 12) & 0xFFF) as u16
}

/// The MIDI note nearest `pitch` (in semitones, fractional) whose pitch
/// class `mask` allows, within 0 to 127. An empty mask allows every note.
pub fn nearest_in(mask: u16, pitch: f32) -> u8 {
    let pitch = if pitch.is_finite() { pitch } else { 60.0 };
    let pitch = pitch.clamp(0.0, 127.0);
    let rounded = pitch.round() as i32;
    if mask & 0xFFF == 0 {
        return rounded as u8;
    }
    let allowed = |note: i32| (0..=127).contains(&note) && mask & 1 << (note % 12) != 0;
    for distance in 0..=12 {
        let (up, down) = (rounded + distance, rounded - distance);
        // The nearer of the two first; above on a tie.
        let up_first = (up as f32 - pitch).abs() <= (pitch - down as f32).abs();
        let (first, second) = if up_first { (up, down) } else { (down, up) };
        for note in [first, second] {
            if allowed(note) {
                return note as u8;
            }
        }
    }
    rounded as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scale_starts_on_its_tonic_and_rises() {
        assert_eq!(SCALE_NAMES.len(), SCALE_DEGREES.len());
        for degrees in SCALE_DEGREES {
            assert_eq!(degrees[0], 0);
            assert!(degrees.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(degrees.iter().all(|&degree| degree < 12));
        }
    }

    #[test]
    fn a_key_moves_the_scale_round_the_octave() {
        let major = strongest_degrees(scale_degrees(0), 12);
        assert_eq!(in_key(major, 0), 0b1010_1011_0101);
        // G major: one sharp, F#.
        let g = in_key(major, 7);
        let classes: Vec<u8> = (0..12).filter(|pc| g & 1 << pc != 0).collect();
        assert_eq!(classes, vec![0, 2, 4, 6, 7, 9, 11]);
    }

    #[test]
    fn a_narrowed_scale_keeps_its_frame() {
        let major = scale_degrees(0);
        assert_eq!(strongest_degrees(major, 1), 1);
        assert_eq!(strongest_degrees(major, 3), 1 | 1 << 7 | 1 << 4);
        let minor = scale_degrees(1);
        assert_eq!(strongest_degrees(minor, 3), 1 | 1 << 7 | 1 << 3);
        assert_eq!(strongest_degrees(major, 0), 1);
    }

    #[test]
    fn the_nearest_note_is_in_the_key() {
        let c_major = in_key(strongest_degrees(scale_degrees(0), 7), 0);
        assert_eq!(nearest_in(c_major, 61.2), 62);
        assert_eq!(nearest_in(c_major, 60.9), 60);
        assert_eq!(nearest_in(c_major, 61.6), 62);
        assert_eq!(nearest_in(c_major, 66.0), 67);
        assert_eq!(nearest_in(c_major, 127.0), 127);
        assert_eq!(nearest_in(1 << 1, 0.0), 1);
        assert_eq!(nearest_in(0, 64.4), 64);
        assert_eq!(nearest_in(c_major, f32::NAN), 60);
        for tenth in 0..1280 {
            let note = nearest_in(c_major, tenth as f32 / 10.0);
            assert!(c_major & 1 << (note % 12) != 0, "{note}");
        }
    }
}
