pub mod modules;
pub mod music;
pub mod patch;
pub mod signal;
pub mod traits;

// Re-export core traits
pub use traits::{validate_port, Generator, ModularModule, Module};

// Re-export signal types
pub use signal::{Audio, AudioSignal, ClockSignal, FrequencySignal, NoteSignal};

// Re-export modules
pub use modules::{
    Adsr, Clock, Dac, MelodyGenerator, MelodyParams, ModulatedOscillator, ModulationInputs,
    Oscillator, OscillatorType, Tempo, Vca,
};

// Re-export patch system
pub use patch::{
    Connection, ModuleConfig, ModuleSpec, Patch, PatchBuilder, PatchRuntime, RunningPatch,
    TimeSignature,
};

// Re-export music theory
pub use music::{Mode, Note, Scale};
