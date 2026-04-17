pub mod dsp;
pub mod factory;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod invention;
pub mod modules;
pub mod music;
pub mod registry;
pub mod scripting;
pub mod traits;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

// Re-export core traits
pub use traits::{
    validate_port, ControlKind, ControlMeta, ControlSurface, ControlValue, Module, SinkModule,
    SinkOutput,
};

// Re-export factory system
pub use factory::{ModuleBuildResult, ModuleFactory};
pub use registry::ModuleRegistry;

// Re-export modules
pub use modules::{
    default_sample_rate, Adsr, AdsrControls, AdsrFactory, AudioBackend, AudioDriver, Clock,
    ClockControls, ClockFactory, CodeControls, CodeFactory, DacFactory, DacModule, Filter,
    FilterControls, FilterFactory, FilterType, Lfo, LfoControls, LfoFactory, MelodyControls,
    MelodyFactory, MelodyGenerator, Mixer, MixerControls, MixerFactory, Oscillator,
    OscillatorControls, OscillatorFactory, OscillatorType, SamplePlayer, SamplePlayerControls,
    SamplePlayerFactory, Vca, VcaControls, VcaFactory,
};

// Re-export invention system
pub use invention::{
    CodeModuleRuntimeInfo, Connection, DevelopmentControl, DevelopmentInput, DevelopmentOutput,
    DevelopmentSpec, GraphCommandError, Invention, InventionBuilder, InventionHandles,
    InventionRuntime, ModuleSpec, OrchestrationRuntime, RenderEngine, RunningInvention,
    RuntimeConnectionInfo, RuntimeController, RuntimeModuleInfo, RuntimeSnapshot, RuntimeState,
    RuntimeStatus, TimeSignature,
};

// Re-export music theory
pub use music::{Note, Scale};
