pub mod agents;
mod atomic;
pub mod dsp;
#[cfg(test)]
mod example_catalog;
pub mod factory;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod invention;
pub mod modules;
pub mod music;
pub mod pkg;
pub mod registry;
pub mod rpc;
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
pub use factory::{GraphModule, ModuleBuildResult, ModuleFactory};
pub use registry::ModuleRegistry;
pub use rpc::{
    validate_schema_version, ModuleTypeInfo, ModuleTypeList, PackageInfo, PackageInstallRequest,
    PackageList, PackageSource, RpcCommand, RpcError, RpcErrorCode, RpcEvent, RpcEventPayload,
    RpcEventSink, RpcRequest, RpcRequestPayload, RpcResponse, RpcResponsePayload,
    RpcSubscriptionTopic, RuntimeControlSnapshot, RuntimeFullSnapshot, RuntimeModuleSnapshot,
    RuntimePortInfo, SinkStatusState, RPC_SCHEMA_VERSION,
};

// Re-export modules
pub use modules::{
    default_sample_rate, Adsr, AdsrControls, AdsrFactory, AgentControls, AgentFactory,
    AudioBackend, AudioDriver, AudioFileSink, AudioFileSinkFactory, AudioFileSinkHandle,
    AudioFileSinkStats, Clock, ClockControls, ClockFactory, CodeControls, CodeFactory, DacFactory,
    DacModule, Filter, FilterControls, FilterFactory, FilterType, Lfo, LfoControls, LfoFactory,
    MelodyControls, MelodyFactory, MelodyGenerator, Mixer, MixerControls, MixerFactory, Oscillator,
    OscillatorControls, OscillatorFactory, OscillatorType, SamplePlayer, SamplePlayerControls,
    SamplePlayerFactory, Vca, VcaControls, VcaFactory,
};

// Re-export invention system
pub use invention::{
    AssetSpec, CodeModuleRuntimeInfo, Connection, DevelopmentControl, DevelopmentInput,
    DevelopmentOutput, DevelopmentSpec, GraphCommandError, Invention, InventionBuilder,
    InventionHandles, InventionRuntime, ModuleSpec, OrchestrationRuntime, RenderEngine,
    RunningInvention, RuntimeConnectionInfo, RuntimeController, RuntimeModuleInfo, RuntimeSnapshot,
    RuntimeState, RuntimeStatus, TimeSignature,
};

// Re-export music theory
pub use music::{Note, Scale};

// Re-export package manifest types
pub use pkg::{
    parse_str as parse_pkg_str, validate as validate_pkg, Author as PkgAuthor,
    Capability as PkgCapability, DepRef as PkgDepRef, EntrySpec as PkgEntrySpec, ManifestError,
    PackageKind, PackageManifest, Requires as PkgRequires, Signing as PkgSigning,
    Target as PkgTarget, ValidationError as PkgValidationError,
};
