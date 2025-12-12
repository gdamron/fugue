pub mod signal;
pub mod module;
pub mod time;
pub mod synthesis;
pub mod scale;
pub mod sequencer;
pub mod modular_audio;

// Re-export signal types
pub use signal::{
    AudioSignal, ControlSignal, GateSignal, TriggerSignal, 
    ClockSignal, FrequencySignal,
};

// Re-export module traits
pub use module::{Module, Generator, Processor, Connect};

pub use time::{Clock, Tempo};
pub use synthesis::{Oscillator, OscillatorType, Filter, Voice};
pub use scale::{Scale, Mode, Note};
pub use sequencer::{MelodyGenerator, MelodyParams, NoteSignal};
pub use modular_audio::Dac;
