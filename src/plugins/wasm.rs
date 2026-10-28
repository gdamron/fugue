//! Wasmtime-backed Fugue module components.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wasmtime::component::{Component, ComponentExportIndex, Instance, Linker, TypedFunc};
use wasmtime::{Config, Engine, Store};

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::pkg::{EntrySpec, PackageManifest};
use crate::{Module, MAX_BLOCK};

use host::{HostState, OutputBlocks};

mod host;
mod world;

const CACHE_VERSION: &str = "v1";

/// Registry factory for manifest-backed Wasm modules.
pub struct WasmModuleFactory;

impl ModuleFactory for WasmModuleFactory {
    fn build_for_inspection(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn Error>> {
        Err("wasm_module inspection requires a running instance; construction may execute guest code".into())
    }

    /// A throwaway check may run the guest's constructor, as a live build
    /// does; a wasm module activates no output when built.
    fn build_for_validation(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn Error>> {
        self.build(sample_rate, config)
    }

    fn type_id(&self) -> &'static str {
        "wasm_module"
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        const { &[crate::module_config::ConfigKey::text("manifest")] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn Error>> {
        let manifest = config
            .get("manifest")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "wasm_module config requires a `manifest` path".to_string())?;
        let module = load_manifest_module(manifest, sample_rate)?;
        Ok(ModuleBuildResult {
            module,
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

/// Loads a `module` package manifest entry as a graph module.
pub fn load_manifest_module(
    manifest_path: impl AsRef<Path>,
    sample_rate: u32,
) -> Result<GraphModule, Box<dyn Error>> {
    let manifest_path = manifest_path.as_ref();
    let manifest = crate::pkg::parse_path(manifest_path)?;
    let EntrySpec::Module { wasm } = &manifest.entry else {
        return Err("manifest is not a module package".into());
    };
    let root = manifest_path
        .parent()
        .ok_or_else(|| "manifest path has no parent directory".to_string())?;
    let wasm_path = root.join(wasm);
    load_component_module(&wasm_path, sample_rate, "{}", &manifest)
}

/// Loads a `.fugue-module.wasm` component as a graph module.
pub fn load_component_module(
    wasm_path: impl AsRef<Path>,
    sample_rate: u32,
    config_json: &str,
    manifest: &PackageManifest,
) -> Result<GraphModule, Box<dyn Error>> {
    let mut module = WasmModule::load(wasm_path.as_ref(), sample_rate, config_json, manifest)?;
    module.init_ports()?;
    Ok(GraphModule::Module(Box::new(module)))
}

/// Guest export `set-input-block(index, values)`.
type SetInputBlockFunc = TypedFunc<(u32, &'static [f32]), (Result<(), String>,)>;
/// Guest exports `set-input(port, value)` and `set-control(key, value)`. The
/// name is lowered from a borrowed `&str`, so a call makes no host `String`.
type SetNamedFunc = TypedFunc<(&'static str, f32), (Result<(), String>,)>;

/// A Fugue `Module` backed by a Wasmtime component instance.
pub struct WasmModule {
    store: Store<HostState>,
    instance: Instance,
    /// The guest's exported `guest` interface, which holds its functions.
    guest: ComponentExportIndex,
    set_input_block: SetInputBlockFunc,
    /// Guest export `process(frames)`, which writes each output into the
    /// store's [`OutputBlocks`] through `emit-output`.
    process: TypedFunc<(u32,), (bool,)>,
    set_input: SetNamedFunc,
    set_input_connected: Option<TypedFunc<(u32, bool), ()>>,
    set_control: SetNamedFunc,
    name: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
    input_names: Vec<&'static str>,
    output_names: Vec<&'static str>,
    input_blocks: Vec<[f32; MAX_BLOCK]>,
    input_connected: Vec<bool>,
    /// Latched after a guest trap; a trapped store fails every subsequent
    /// call, so we stop calling into it instead of paying full call cost
    /// per block.
    failed: bool,
}

impl WasmModule {
    fn load(
        wasm_path: &Path,
        sample_rate: u32,
        config_json: &str,
        manifest: &PackageManifest,
    ) -> Result<Self, Box<dyn Error>> {
        let engine = component_engine()?;
        let component = cached_component(&engine, wasm_path)?;
        let guest = world::check_world(&engine, &component)?;
        let export = |name: &str| {
            component
                .get_export_index(Some(&guest), name)
                .ok_or_else(|| missing_export(name))
        };
        let mut linker = Linker::<HostState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        host::add_to_linker(&mut linker)?;

        let mut store = Store::new(&engine, HostState::from_manifest(manifest)?);
        store.set_fuel(u64::MAX / 2)?;
        let instance = linker.instantiate(&mut store, &component)?;

        let init = instance
            .get_typed_func::<(u32, String), (Result<(), String>,)>(&mut store, export("init")?)?;
        let (result,) = init.call(&mut store, (sample_rate, config_json.to_string()))?;
        result.map_err(|e| format!("wasm module init failed: {e}"))?;

        let set_input_block = instance.get_typed_func(&mut store, export("set-input-block")?)?;
        let process = instance.get_typed_func(&mut store, export("process")?)?;
        let set_input = instance.get_typed_func(&mut store, export("set-input")?)?;
        // Optional, but a mistyped export fails loudly instead of being skipped.
        let set_input_connected = match export("set-input-connected") {
            Ok(index) => Some(instance.get_typed_func(&mut store, index)?),
            Err(_) => None,
        };
        let set_control = instance.get_typed_func(&mut store, export("set-control")?)?;

        Ok(Self {
            store,
            instance,
            guest,
            set_input_block,
            process,
            set_input,
            set_input_connected,
            set_control,
            name: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_names: Vec::new(),
            output_names: Vec::new(),
            input_blocks: Vec::new(),
            input_connected: Vec::new(),
            failed: false,
        })
    }

    fn init_ports(&mut self) -> Result<(), Box<dyn Error>> {
        self.name = self.call_string("name")?;
        self.inputs = self.call_string_list("inputs")?;
        self.outputs = self.call_string_list("outputs")?;
        self.input_blocks = vec![[0.0; MAX_BLOCK]; self.inputs.len()];
        self.store.data_mut().outputs = OutputBlocks::new(self.outputs.len());
        self.input_connected = vec![false; self.inputs.len()];

        self.input_names = leak_port_names(&self.inputs);
        self.output_names = leak_port_names(&self.outputs);
        for port in self.inputs.iter().chain(&self.outputs) {
            let problems = crate::interface_naming::naming_problems(port);
            if !problems.is_empty() {
                eprintln!(
                    "wasm module '{}': port '{port}' does not follow the interface convention: it {}",
                    self.name,
                    problems.join("; it ")
                );
            }
        }
        Ok(())
    }

    fn guest_export(&mut self, name: &str) -> Result<ComponentExportIndex, String> {
        self.instance
            .get_export_index(&mut self.store, Some(&self.guest), name)
            .ok_or_else(|| missing_export(name))
    }

    fn call_string(&mut self, export: &str) -> Result<String, Box<dyn Error>> {
        let export = self.guest_export(export)?;
        let func = self
            .instance
            .get_typed_func::<(), (String,)>(&mut self.store, export)?;
        let (value,) = func.call(&mut self.store, ())?;
        Ok(value)
    }

    fn call_string_list(&mut self, export: &str) -> Result<Vec<String>, Box<dyn Error>> {
        let export = self.guest_export(export)?;
        let func = self
            .instance
            .get_typed_func::<(), (Vec<String>,)>(&mut self.store, export)?;
        let (value,) = func.call(&mut self.store, ())?;
        Ok(value)
    }

    /// Latches the module as failed after a guest trap or error, zeroing all
    /// outputs so downstream modules don't hold a stale sustained value.
    fn fail(&mut self, export: &str, error: &dyn std::fmt::Display) -> bool {
        eprintln!("{}", failure_message(&self.name, export, error));
        for block in &mut self.store.data_mut().outputs.blocks {
            block.fill(0.0);
        }
        self.failed = true;
        false
    }
}

impl Module for WasmModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, frames: usize) -> bool {
        if self.failed {
            return false;
        }
        let frames = frames.min(MAX_BLOCK);
        for index in 0..self.input_blocks.len() {
            if !self.input_connected.get(index).copied().unwrap_or(false) {
                continue;
            }
            // Lowered straight from the input block: no host-side copy.
            let values = &self.input_blocks[index][..frames];
            match self
                .set_input_block
                .call(&mut self.store, (index as u32, values))
            {
                Ok((Ok(()),)) => {}
                Ok((Err(error),)) => return self.fail("set-input-block", &error),
                Err(error) => return self.fail("set-input-block", &error),
            }
        }

        // The guest writes each output through `emit-output`, which copies
        // it into the store's blocks: nothing is lifted into a host `Vec`.
        self.store.data_mut().outputs.begin(frames);
        let processed = self.process.call(&mut self.store, (frames as u32,));
        self.store.data_mut().outputs.end();
        match processed {
            Ok((active,)) => active,
            Err(error) => self.fail("process", &error),
        }
    }

    fn inputs(&self) -> &[&str] {
        &self.input_names
    }

    fn outputs(&self) -> &[&str] {
        &self.output_names
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.input_blocks[index]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.store.data().outputs.blocks[index]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let (result,) = self
            .set_input
            .call(&mut self.store, (port, value))
            .map_err(|e| e.to_string())?;
        result
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = self
            .outputs
            .iter()
            .position(|name| name == port)
            .ok_or_else(|| format!("Unknown output port: {port}"))?;
        Ok(self.store.data().outputs.blocks[index][0])
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        if let Some(slot) = self.input_connected.get_mut(index) {
            *slot = connected;
        }
        if let Some(func) = &self.set_input_connected {
            let _ = func.call(&mut self.store, (index as u32, connected));
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        let (result,) = self
            .set_control
            .call(&mut self.store, (key, value))
            .map_err(|e| e.to_string())?;
        result
    }
}

/// Names the guest's reason: `{error:#}` prints wasmtime's whole context
/// chain, whose outermost link is only the wasm backtrace.
fn failure_message(name: &str, export: &str, error: &dyn std::fmt::Display) -> String {
    format!("wasm module '{name}' failed in `{export}`: {error:#}; muting output")
}

fn component_engine() -> Result<Engine, Box<dyn Error>> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    // The host makes only sync calls. With concurrency support on (the
    // default when wasmtime's `component-model-async` feature is compiled
    // in), every call books a guest task: boxed closures and a hash-set
    // insert, allocated and freed on the audio thread.
    config.concurrency_support(false);
    config.consume_fuel(true);
    Ok(Engine::new(&config)?)
}

fn cached_component(engine: &Engine, wasm_path: &Path) -> Result<Component, Box<dyn Error>> {
    let bytes = fs::read(wasm_path)?;
    let cache_path = cache_path_for(engine, wasm_path, &bytes)?;
    if cache_path.exists() {
        // The cache file is produced by this engine configuration and content
        // hash; if it fails wasmtime's own compatibility check (e.g. after a
        // dependency bump), fall through and recompile instead of erroring.
        if let Ok(component) = unsafe { Component::deserialize_file(engine, &cache_path) } {
            return Ok(component);
        }
    }

    let compiled = engine.precompile_component(&bytes)?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&cache_path, compiled)?;
    let component = unsafe { Component::deserialize_file(engine, &cache_path)? };
    Ok(component)
}

fn cache_path_for(
    engine: &Engine,
    wasm_path: &Path,
    wasm: &[u8],
) -> Result<PathBuf, Box<dyn Error>> {
    let mut hasher = Sha256::new();
    hasher.update(CACHE_VERSION.as_bytes());
    hasher.update(engine_compatibility_tag(engine).to_le_bytes());
    hasher.update(wasm_path.to_string_lossy().as_bytes());
    hasher.update(wasm);
    let digest = hasher.finalize();
    let file = format!("{}.cwasm", crate::hex::lower_hex(&digest));
    Ok(fugue_cache_dir()?.join(file))
}

/// Hashes the engine's precompile compatibility state (wasmtime version,
/// compiler config, target) so cache keys rotate automatically on dependency
/// bumps instead of relying on a hardcoded version tag.
fn engine_compatibility_tag(engine: &Engine) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
}

fn fugue_cache_dir() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".fugue/cache/wasm"))
}

/// Interns port names in a global set so repeated load/unload of the same
/// module reuses one leaked allocation per distinct name instead of growing
/// without bound. Called at module init only, never on the audio thread.
fn leak_port_names(names: &[String]) -> Vec<&'static str> {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut interned = INTERNED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .expect("port name intern lock");
    names
        .iter()
        .map(|name| match interned.get(name.as_str()) {
            Some(existing) => *existing,
            None => {
                let leaked = Box::leak(name.clone().into_boxed_str()) as &'static str;
                interned.insert(leaked);
                leaked
            }
        })
        .collect()
}

fn missing_export(name: &str) -> String {
    format!(
        "wasm module does not export `{name}` from {}",
        world::GUEST_INTERFACE
    )
}

#[cfg(test)]
pub(crate) mod alloc_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_changes_with_wasm_bytes() {
        let engine = component_engine().expect("engine");
        let path = Path::new("module.fugue-module.wasm");
        let a = cache_path_for(&engine, path, b"a").expect("cache path");
        let b = cache_path_for(&engine, path, b"b").expect("cache path");
        assert_ne!(a, b);
    }
}
