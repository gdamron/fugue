#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Ionian, // Major
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Aeolian, // Natural Minor
    Locrian,
}

impl Mode {
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
