use crate::signal::{Audio, FrequencySignal};

/// A combined gate and frequency signal representing a musical note.
///
/// Used to communicate note-on/off state and pitch information
/// between sequencers and voices.
#[derive(Debug, Clone, Copy)]
pub struct NoteSignal {
    /// Gate signal with envelope/velocity information.
    pub gate: Audio,
    /// The pitch of the note.
    pub frequency: FrequencySignal,
}
