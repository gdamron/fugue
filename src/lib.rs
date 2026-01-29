pub mod modules;
pub mod music;
pub mod patch;
pub mod traits;

// Re-export core traits
pub use traits::{validate_port, Module};

// Re-export modules
pub use modules::{
    Adsr, Clock, Dac, MelodyGenerator, MelodyParams, Oscillator, OscillatorType, Tempo, Vca,
};

// Re-export patch system
pub use patch::{
    Connection, ModuleConfig, ModuleSpec, Patch, PatchBuilder, PatchRuntime, RunningPatch,
    TimeSignature,
};

// Re-export music theory
pub use music::{Mode, Note, Scale};
