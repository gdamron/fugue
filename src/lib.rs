pub mod factory;
pub mod modules;
pub mod music;
pub mod patch;
pub mod registry;
pub mod traits;

// Re-export core traits
pub use traits::{validate_port, Module, SinkModule, SinkOutput};

// Re-export factory system
pub use factory::{ModuleBuildResult, ModuleFactory};
pub use registry::ModuleRegistry;

// Re-export modules
pub use modules::{
    Adsr, AdsrFactory, AudioBackend, AudioDriver, Clock, ClockFactory, DacFactory, DacModule,
    Filter, FilterFactory, FilterType, Lfo, LfoFactory, MelodyFactory, MelodyGenerator,
    MelodyParams, Oscillator, OscillatorFactory, OscillatorType, Tempo, Vca, VcaFactory,
};

// Re-export patch system
pub use patch::{
    Connection, ModuleSpec, Patch, PatchBuilder, PatchHandles, PatchRuntime, RunningPatch,
    TimeSignature,
};

// Re-export music theory
pub use music::{Mode, Note, Scale};
