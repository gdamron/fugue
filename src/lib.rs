pub mod builder;
pub mod modular_audio;
pub mod module;
pub mod oscillator;
pub mod oscillator_builder;
pub mod patch;
pub mod scale;
pub mod sequencer;
pub mod signal;
pub mod synthesis;
pub mod time;

// Re-export signal types
pub use signal::{Audio, ClockSignal, Control, FrequencySignal};

// Legacy aliases for backward compatibility
pub use signal::{AudioSignal, ControlSignal, GateSignal, TriggerSignal};

// Re-export module traits
pub use module::{Connect, Generator, Module, Processor};

pub use builder::{PatchBuilder, PatchRuntime, RunningPatch};
pub use modular_audio::Dac;
pub use oscillator::{ModulatedOscillator, ModulationInputs, Oscillator, OscillatorType};
pub use oscillator_builder::{
    OscillatorPatchBuilder, OscillatorPatchRuntime, RunningOscillatorPatch,
};
pub use patch::{Connection, ModuleConfig, ModuleSpec, Patch, TimeSignature};
pub use scale::{Mode, Note, Scale};
pub use sequencer::{MelodyGenerator, MelodyParams, NoteSignal};
pub use synthesis::{Filter, Mixer, Voice};
pub use time::{Clock, Tempo};
