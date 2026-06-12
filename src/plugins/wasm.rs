//! Wasmtime-backed Fugue module components.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Instance, Linker, ResourceTable, TypedFunc};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::pkg::{Capability, EntrySpec, PackageManifest};
use crate::{Module, MAX_BLOCK};

const CACHE_VERSION: &str = "v1";

/// Registry factory for manifest-backed Wasm modules.
pub struct WasmModuleFactory;

impl ModuleFactory for WasmModuleFactory {
    fn type_id(&self) -> &'static str {
        "wasm_module"
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

/// A Fugue `Module` backed by a Wasmtime component instance.
pub struct WasmModule {
    store: Store<HostState>,
    instance: Instance,
    set_input_block: TypedFunc<(u32, &'static [f32]), (Result<(), String>,)>,
    process: TypedFunc<(u32,), (bool,)>,
    process_output_block: Option<TypedFunc<(u32,), (Result<(bool, Vec<f32>), String>,)>>,
    output_block: TypedFunc<(u32,), (Result<Vec<f32>, String>,)>,
    set_input: TypedFunc<(String, f32), (Result<(), String>,)>,
    set_input_connected: Option<TypedFunc<(u32, bool), ()>>,
    set_control: TypedFunc<(String, f32), (Result<(), String>,)>,
    name: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
    input_names: Vec<&'static str>,
    output_names: Vec<&'static str>,
    input_blocks: Vec<[f32; MAX_BLOCK]>,
    output_blocks: Vec<[f32; MAX_BLOCK]>,
    input_transfer: Vec<Vec<f32>>,
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
        let mut linker = Linker::<HostState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;

        let mut store = Store::new(&engine, HostState::from_manifest(manifest)?);
        store.set_fuel(u64::MAX / 2)?;
        let instance = linker.instantiate(&mut store, &component)?;

        let init =
            instance.get_typed_func::<(u32, String), (Result<(), String>,)>(&mut store, "init")?;
        let (result,) = init.call(&mut store, (sample_rate, config_json.to_string()))?;
        result.map_err(|e| format!("wasm module init failed: {e}"))?;

        let set_input_block = instance.get_typed_func(&mut store, "set-input-block")?;
        let process = instance.get_typed_func(&mut store, "process")?;
        let process_output_block =
            optional_typed_func(&instance, &mut store, "process-output-block")?;
        let output_block = instance.get_typed_func(&mut store, "output-block")?;
        let set_input = instance.get_typed_func(&mut store, "set-input")?;
        let set_input_connected =
            optional_typed_func(&instance, &mut store, "set-input-connected")?;
        let set_control = instance.get_typed_func(&mut store, "set-control")?;

        Ok(Self {
            store,
            instance,
            set_input_block,
            process,
            process_output_block,
            output_block,
            set_input,
            set_input_connected,
            set_control,
            name: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_names: Vec::new(),
            output_names: Vec::new(),
            input_blocks: Vec::new(),
            output_blocks: Vec::new(),
            input_transfer: Vec::new(),
            input_connected: Vec::new(),
            failed: false,
        })
    }

    fn init_ports(&mut self) -> Result<(), Box<dyn Error>> {
        self.name = self.call_string("name")?;
        self.inputs = self.call_string_list("inputs")?;
        self.outputs = self.call_string_list("outputs")?;
        self.input_blocks = vec![[0.0; MAX_BLOCK]; self.inputs.len()];
        self.output_blocks = vec![[0.0; MAX_BLOCK]; self.outputs.len()];
        self.input_transfer = (0..self.inputs.len())
            .map(|_| Vec::with_capacity(MAX_BLOCK))
            .collect();
        self.input_connected = vec![false; self.inputs.len()];

        self.input_names = leak_port_names(&self.inputs);
        self.output_names = leak_port_names(&self.outputs);
        Ok(())
    }

    fn call_string(&mut self, export: &str) -> Result<String, Box<dyn Error>> {
        let func = self
            .instance
            .get_typed_func::<(), (String,)>(&mut self.store, export)?;
        let (value,) = func.call(&mut self.store, ())?;
        Ok(value)
    }

    fn call_string_list(&mut self, export: &str) -> Result<Vec<String>, Box<dyn Error>> {
        let func = self
            .instance
            .get_typed_func::<(), (Vec<String>,)>(&mut self.store, export)?;
        let (value,) = func.call(&mut self.store, ())?;
        Ok(value)
    }

    /// Latches the module as failed after a guest trap or error, zeroing all
    /// outputs so downstream modules don't hold a stale sustained value.
    fn fail(&mut self, export: &str) -> bool {
        eprintln!(
            "wasm module '{}' failed in `{export}`; muting output",
            self.name
        );
        for block in &mut self.output_blocks {
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
            let values = &mut self.input_transfer[index];
            values.clear();
            values.extend_from_slice(&self.input_blocks[index][..frames]);
            let Ok((Ok(()),)) = self
                .set_input_block
                .call(&mut self.store, (index as u32, &*values))
            else {
                return self.fail("set-input-block");
            };
        }

        // Note: each block lifts the guest's `list<f32>` into a fresh `Vec`,
        // an allocation inherent to component-model lifting. A shared-memory
        // output path is the planned follow-up to make this hot path
        // allocation-free.
        if self.output_blocks.len() == 1 {
            if let Some(process_output_block) = &self.process_output_block {
                let Ok((Ok((active, values)),)) =
                    process_output_block.call(&mut self.store, (frames as u32,))
                else {
                    return self.fail("process-output-block");
                };
                let n = frames.min(values.len());
                self.output_blocks[0][..n].copy_from_slice(&values[..n]);
                if n < frames {
                    self.output_blocks[0][n..frames].fill(0.0);
                }
                return active;
            }
        }

        let Ok((active,)) = self.process.call(&mut self.store, (frames as u32,)) else {
            return self.fail("process");
        };

        for index in 0..self.output_blocks.len() {
            let Ok((Ok(values),)) = self.output_block.call(&mut self.store, (index as u32,)) else {
                return self.fail("output-block");
            };
            let n = frames.min(values.len());
            self.output_blocks[index][..n].copy_from_slice(&values[..n]);
            if n < frames {
                self.output_blocks[index][n..frames].fill(0.0);
            }
        }

        active
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
        &self.output_blocks[index]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let (result,) = self
            .set_input
            .call(&mut self.store, (port.to_string(), value))
            .map_err(|e| e.to_string())?;
        result
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = self
            .outputs
            .iter()
            .position(|name| name == port)
            .ok_or_else(|| format!("Unknown output port: {port}"))?;
        Ok(self.output_blocks[index][0])
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
            .call(&mut self.store, (key.to_string(), value))
            .map_err(|e| e.to_string())?;
        result
    }
}

fn component_engine() -> Result<Engine, Box<dyn Error>> {
    let mut config = Config::new();
    config.wasm_component_model(true);
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
    let file = format!("{digest:x}.cwasm");
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

/// Resolves an optional export, distinguishing "not exported" (None) from
/// "exported with the wrong signature" (error) so a mistyped fast-path export
/// fails loudly instead of silently falling back to the slow path.
fn optional_typed_func<Params, Results>(
    instance: &Instance,
    store: &mut Store<HostState>,
    name: &str,
) -> Result<Option<TypedFunc<Params, Results>>, Box<dyn Error>>
where
    Params: wasmtime::component::ComponentNamedList + wasmtime::component::Lower,
    Results: wasmtime::component::ComponentNamedList + wasmtime::component::Lift,
{
    if instance.get_func(&mut *store, name).is_none() {
        return Ok(None);
    }
    Ok(Some(instance.get_typed_func(&mut *store, name)?))
}

struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl HostState {
    fn from_manifest(manifest: &PackageManifest) -> Result<Self, Box<dyn Error>> {
        let mut builder = WasiCtx::builder();
        for cap in &manifest.requires.capabilities {
            match Capability::parse(cap).ok_or_else(|| format!("invalid capability: {cap}"))? {
                Capability::Random | Capability::Time => {}
                Capability::FsRead(scope) => {
                    preopen(&mut builder, &scope, false)?;
                }
                Capability::FsWrite(scope) => {
                    preopen(&mut builder, &scope, true)?;
                }
                Capability::Net(host) => {
                    return Err(format!(
                        "network capability is not supported for wasm modules yet: {host}"
                    )
                    .into());
                }
            }
        }

        Ok(Self {
            table: ResourceTable::new(),
            wasi: builder.build(),
        })
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn preopen(
    builder: &mut wasmtime_wasi::WasiCtxBuilder,
    scope: &str,
    writable: bool,
) -> Result<(), Box<dyn Error>> {
    let path = Path::new(scope);
    let perms = if writable {
        wasmtime_wasi::DirPerms::all()
    } else {
        wasmtime_wasi::DirPerms::READ
    };
    let file_perms = if writable {
        wasmtime_wasi::FilePerms::all()
    } else {
        wasmtime_wasi::FilePerms::READ
    };
    builder.preopened_dir(path, scope, perms, file_perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::{Author, EntrySpec, PackageKind, Requires, Target};

    fn manifest_with_caps(capabilities: Vec<String>) -> PackageManifest {
        PackageManifest {
            id: "fugue.test.module".to_string(),
            version: "1.0.0".to_string(),
            kind: PackageKind::Module,
            license: "MIT".to_string(),
            authors: vec![Author {
                name: "Fugue Test".to_string(),
                url: None,
            }],
            description: None,
            homepage: None,
            targets: vec![Target::InGraphAgent],
            requires: Requires {
                mcp_tools: Vec::new(),
                capabilities,
            },
            deps: Vec::new(),
            entry: EntrySpec::Module {
                wasm: "module.fugue-module.wasm".to_string(),
            },
            signing: None,
        }
    }

    #[test]
    fn host_state_rejects_unsupported_network_capability() {
        let manifest = manifest_with_caps(vec!["net:example.com".to_string()]);
        let err = match HostState::from_manifest(&manifest) {
            Ok(_) => panic!("network capability should be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("network capability is not supported"));
    }

    #[test]
    fn host_state_accepts_random_and_time_without_extra_grants() {
        let manifest = manifest_with_caps(vec!["random".to_string(), "time".to_string()]);
        HostState::from_manifest(&manifest).expect("random/time capabilities are valid");
    }

    #[test]
    fn cache_path_changes_with_wasm_bytes() {
        let engine = component_engine().expect("engine");
        let path = Path::new("module.fugue-module.wasm");
        let a = cache_path_for(&engine, path, b"a").expect("cache path");
        let b = cache_path_for(&engine, path, b"b").expect("cache path");
        assert_ne!(a, b);
    }
}
