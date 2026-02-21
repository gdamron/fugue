pub mod factory;
pub mod invention;
pub mod modules;
pub mod music;
pub mod registry;
pub mod traits;

// Re-export core traits
pub use traits::{validate_port, ControlMeta, Module, SinkModule, SinkOutput};

// Re-export factory system
pub use factory::{ModuleBuildResult, ModuleFactory};
pub use registry::ModuleRegistry;

// Re-export modules
pub use modules::{
    default_sample_rate, Adsr, AdsrControls, AdsrFactory, AudioBackend, AudioDriver, Clock,
    ClockControls, ClockFactory, DacFactory, DacModule, Filter, FilterControls, FilterFactory,
    FilterType, Lfo, LfoControls, LfoFactory, MelodyControls, MelodyFactory, MelodyGenerator,
    Mixer, MixerControls, MixerFactory, Oscillator, OscillatorControls, OscillatorFactory,
    OscillatorType, Vca, VcaControls, VcaFactory,
};

// Deprecated type aliases for backward compatibility
#[allow(deprecated)]
pub use modules::{MelodyParams, Tempo};

// Re-export invention system
pub use invention::{
    Connection, Invention, InventionBuilder, InventionHandles, InventionRuntime, ModuleSpec,
    RunningInvention, TimeSignature,
};

// Re-export music theory
pub use music::{Mode, Note, Scale};
