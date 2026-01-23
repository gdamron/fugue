use crate::signal::{Audio, FrequencySignal};

/// Output combines gate and frequency information
#[derive(Debug, Clone, Copy)]
pub struct NoteSignal {
    pub gate: Audio,
    pub frequency: FrequencySignal,
}
