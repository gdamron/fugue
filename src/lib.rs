pub mod modular_audio;
pub mod module;
pub mod oscillator;
pub mod scale;
pub mod sequencer;
pub mod signal;
pub mod synthesis;
pub mod time;

// Re-export signal types
pub use signal::{Audio, Control, ClockSignal, FrequencySignal};

// Legacy aliases for backward compatibility
pub use signal::{AudioSignal, ControlSignal, GateSignal, TriggerSignal};

// Re-export module traits
pub use module::{Connect, Generator, Module, Processor};

pub use modular_audio::Dac;
pub use oscillator::{Oscillator, OscillatorType};
pub use scale::{Mode, Note, Scale};
pub use sequencer::{MelodyGenerator, MelodyParams, NoteSignal};
pub use synthesis::{Filter, Voice};
pub use time::{Clock, Tempo};
