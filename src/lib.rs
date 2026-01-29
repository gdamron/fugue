pub mod modular_audio;
pub mod modular_builder;
pub mod module;
pub mod oscillator;
pub mod patch;
pub mod scale;
pub mod sequencer;
pub mod signal;
pub mod synthesis;
pub mod time;

// Re-export signal types
pub use signal::{Audio, ClockSignal, FrequencySignal};

// Legacy alias for backward compatibility
pub use signal::AudioSignal;

// Re-export module traits
pub use module::{Generator, ModularModule, Module, Processor};

pub use modular_audio::Dac;
pub use modular_builder::{ModularPatchBuilder, ModularPatchRuntime, RunningModularPatch};
pub use oscillator::{ModulatedOscillator, ModulationInputs, Oscillator, OscillatorType};
pub use patch::{ModuleConfig, ModuleSpec, Patch, TimeSignature};
pub use scale::{Mode, Note, Scale};
pub use sequencer::{MelodyGenerator, MelodyParams, NoteSignal};
pub use synthesis::{ModularAdsr, Vca};
pub use time::{Clock, Tempo};
