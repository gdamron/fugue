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
#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
pub mod plugins;
pub mod registry;
pub mod rpc;
pub mod scripting;
pub(crate) mod streaming;
pub mod traits;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

// Re-export core traits
pub use traits::{
    validate_port, ControlKind, ControlMeta, ControlSurface, ControlValue, Module, SinkModule,
    SinkOutput, DEFAULT_BLOCK_SIZE, MAX_BLOCK,
};

// Re-export factory system
pub use factory::{GraphModule, ModuleBuildResult, ModuleFactory};
pub use registry::ModuleRegistry;
pub use rpc::{
    validate_schema_version, ModuleTypeInfo, ModuleTypeList, PackageInfo, PackageInstallRequest,
    PackageList, PackageSource, ReloadMode, ReloadOutcome, RpcCommand, RpcError, RpcErrorCode,
    RpcEvent, RpcEventPayload, RpcEventSink, RpcRequest, RpcRequestPayload, RpcResponse,
    RpcResponsePayload, RpcSubscriptionTopic, RuntimeControlSnapshot, RuntimeFullSnapshot,
    RuntimeModuleSnapshot, RuntimePortInfo, SaveReport, SinkStatusState, RPC_SCHEMA_VERSION,
};

// Re-export modules
pub use modules::{
    default_sample_rate, Adsr, AdsrControls, AdsrFactory, AgentControls, AgentFactory,
    AudioBackend, AudioDiagnostics, AudioDiagnosticsSnapshot, AudioDriver, AudioFileSink,
    AudioFileSinkFactory, AudioFileSinkHandle, AudioFileSinkStats, Clock, ClockControls,
    ClockFactory, CodeControls, CodeFactory, ControlScheduler, ControlSchedulerControls,
    ControlSchedulerFactory, DacFactory, DacModule, Filter, FilterControls, FilterFactory,
    FilterType, Lfo, LfoControls, LfoFactory, MelodyControls, MelodyFactory, MelodyGenerator,
    Mixer, MixerControls, MixerFactory, Oscillator, OscillatorControls, OscillatorFactory,
    OscillatorType, SampleKit, SampleKitControls, SampleKitFactory, SamplePlayer,
    SamplePlayerControls, SamplePlayerFactory, Vca, VcaControls, VcaFactory,
};

#[cfg(not(target_arch = "wasm32"))]
pub use modules::{
    RtmpSink, RtmpSinkConfig, RtmpSinkFactory, RtmpSinkHandle, RtmpSinkStats, YoutubeSink,
    YoutubeSinkFactory, YoutubeSinkHandle, YoutubeSinkStats,
};

// Re-export invention system
pub use invention::{
    validate_score, AssetSpec, CodeModuleRuntimeInfo, Connection, ControlOverride,
    DevelopmentControl, DevelopmentDefinitions, DevelopmentInput, DevelopmentOutput,
    DevelopmentSpec, GraphCommandError, Invention, InventionBuilder, InventionHandles,
    InventionRuntime, ModuleSpec, OrchestrationRuntime, ReloadError, ReloadReport, RenderEngine,
    RunningInvention, RuntimeConnectionInfo, RuntimeController, RuntimeModuleInfo, RuntimeSnapshot,
    RuntimeState, RuntimeStatus, Score, TimeSignature, SCORE_SCHEMA_V1,
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

// Re-export sample-pack entry types
pub use pkg::{
    parse_sample_pack_str, validate_sample_pack, SampleFile, SamplePackError, SamplePackManifest,
    SamplePackValidationError, SampleSlice,
};

// Re-export audio asset reference types
#[cfg(not(target_arch = "wasm32"))]
pub use pkg::{default_packages_dir, resolve_package_asset, ResolvedPackageAsset};
pub use pkg::{AudioAssetRef, PackageAudioRef};

// Re-export lockfile types
#[cfg(not(target_arch = "wasm32"))]
pub use pkg::compute_integrity;
pub use pkg::{LockError, LockSource, LockedPackage, Lockfile, LOCKFILE_NAME, LOCKFILE_VERSION};

#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
pub use plugins::wasm::{
    load_component_module, load_manifest_module, WasmModule, WasmModuleFactory,
};
