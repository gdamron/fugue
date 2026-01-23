/// A musical mode defining the interval pattern of a scale.
///
/// Each mode has a unique pattern of whole and half steps that gives it
/// a distinctive sound character.
#[derive(Debug, Clone, Copy)]
pub enum Mode {
    /// Major scale (W-W-H-W-W-W-H).
    Ionian,
    /// Minor with raised 6th, jazzy sound.
    Dorian,
    /// Minor with lowered 2nd, Spanish/Middle Eastern flavor.
    Phrygian,
    /// Major with raised 4th, bright and dreamy.
    Lydian,
    /// Major with lowered 7th, bluesy dominant sound.
    Mixolydian,
    /// Natural minor scale (W-H-W-W-H-W-W).
    Aeolian,
    /// Diminished sound, rare and unstable.
    Locrian,
}

impl Mode {
    /// Returns the semitone intervals from the root for each scale degree.
    ///
    /// The returned slice contains 7 values representing the semitone offset
    /// from the root note for degrees 1-7 of the scale.
    pub fn intervals(&self) -> &[i32] {
        match self {
            Mode::Ionian => &[0, 2, 4, 5, 7, 9, 11],
            Mode::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Mode::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Mode::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Mode::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Mode::Aeolian => &[0, 2, 3, 5, 7, 8, 10],
            Mode::Locrian => &[0, 1, 3, 5, 6, 8, 10],
        }
    }
}
