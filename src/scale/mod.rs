mod mode;
mod note;

pub use mode::Mode;
pub use note::Note;

pub struct Scale {
    root: Note,
    mode: Mode,
}

impl Scale {
    pub fn new(root: Note, mode: Mode) -> Self {
        Self { root, mode }
    }

    pub fn get_note(&self, degree: usize) -> Note {
        let intervals = self.mode.intervals();
        let octave = degree / intervals.len();
        let degree_in_octave = degree % intervals.len();

        let midi_note =
            self.root.midi_note as i32 + (octave as i32 * 12) + intervals[degree_in_octave];

        Note::new(midi_note as u8)
    }

    pub fn degrees_in_octave(&self) -> usize {
        self.mode.intervals().len()
    }
}
