//! Note values a module can be timed in at a tempo: the `division`
//! choices shared by the `delay`, the `lfo` and the `phaser`.

/// The note values `division` offers, `off` first, in the music module's
/// vocabulary (`8th_note`, `quarter_triplet`).
pub(crate) const DIVISIONS: &[&str] = &[
    "off",
    "whole_note",
    "half_note",
    "quarter_note",
    "8th_note",
    "16th_note",
    "32nd_note",
    "dotted_half_note",
    "dotted_quarter_note",
    "dotted_8th_note",
    "dotted_16th_note",
    "half_triplet",
    "quarter_triplet",
    "8th_triplet",
    "16th_triplet",
];

/// Each division's length in beats (a quarter note is one), in
/// [`DIVISIONS`] order; `off` has none.
pub(crate) const DIVISION_BEATS: [f64; 15] = [
    0.0,
    4.0,
    2.0,
    1.0,
    0.5,
    0.25,
    0.125,
    3.0,
    1.5,
    0.75,
    0.375,
    4.0 / 3.0,
    2.0 / 3.0,
    1.0 / 3.0,
    1.0 / 6.0,
];

/// The fractions musicians write, as other spellings of the divisions.
pub(crate) const DIVISION_SPELLINGS: &[(&str, &str)] = &[
    ("1/1", "whole_note"),
    ("1/2", "half_note"),
    ("1/4", "quarter_note"),
    ("1/8", "8th_note"),
    ("1/16", "16th_note"),
    ("1/32", "32nd_note"),
    ("1/2.", "dotted_half_note"),
    ("1/4.", "dotted_quarter_note"),
    ("1/8.", "dotted_8th_note"),
    ("1/16.", "dotted_16th_note"),
    ("1/2t", "half_triplet"),
    ("1/4t", "quarter_triplet"),
    ("1/8t", "8th_triplet"),
    ("1/16t", "16th_triplet"),
];

/// The length in seconds of the division at `position` at `bpm`, or `None`
/// for `off` (or a position past the list).
pub(crate) fn division_seconds(position: usize, bpm: f64) -> Option<f64> {
    match DIVISION_BEATS.get(position) {
        Some(&beats) if beats > 0.0 => Some(beats * 60.0 / bpm),
        _ => None,
    }
}

/// A patched tempo when it is usable (finite, above 0), held to 1–1000
/// BPM; otherwise `control`.
pub(crate) fn tempo(patched: Option<f32>, control: f32) -> f64 {
    match patched {
        Some(bpm) if bpm.is_finite() && bpm > 0.0 => f64::from(bpm.clamp(1.0, 1000.0)),
        _ => f64::from(control),
    }
}
