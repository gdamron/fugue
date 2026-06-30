//! Invention builder for creating modular synthesis setups.

use crate::invention::format::Invention;
use crate::invention::handles::InventionHandles;
use crate::invention::runtime::{
    validate_input_port, validate_output_port, ControlSurfaceInstance, InventionRuntime,
    ModuleInstance,
};
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState};
use crate::registry::ModuleRegistry;
use crate::{GraphModule, ModuleFactory};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::development::DevelopmentFactory;
use super::graph::RoutingConnection;

/// Result type for building modules, containing modules, sinks, and handles.
type BuildModulesResult = (
    IndexMap<String, ModuleInstance>,
    Vec<String>,
    IndexMap<String, ControlSurfaceInstance>,
    InventionHandles,
);

/// Invention builder that uses named port routing.
///
/// Modules are connected via explicit port names rather than type-based routing.
/// The builder uses a registry to construct modules from their type names.
pub struct InventionBuilder {
    sample_rate: u32,
    registry: ModuleRegistry,
    registered: Arc<Mutex<HashSet<String>>>,
}

impl InventionBuilder {
    /// Creates a new invention builder with the default module registry.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            registry: ModuleRegistry::default(),
            registered: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Creates a new invention builder with a custom module registry.
    pub fn with_registry(sample_rate: u32, registry: ModuleRegistry) -> Self {
        Self {
            sample_rate,
            registry,
            registered: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Creates a new invention builder sharing the given registered-developments set.
    /// Used internally by `DevelopmentFactory` so nested builds share the same guard.
    pub(crate) fn with_registry_and_registered(
        sample_rate: u32,
        registry: ModuleRegistry,
        registered: Arc<Mutex<HashSet<String>>>,
    ) -> Self {
        Self {
            sample_rate,
            registry,
            registered,
        }
    }

    /// Builds and prepares an invention for execution.
    ///
    /// Returns both the runtime (for starting audio) and handles (for runtime control).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let invention = Invention::from_file("my_invention.json")?;
    /// let builder = InventionBuilder::new(44100);
    /// let (runtime, handles) = builder.build(invention)?;
    ///
    /// // Get control handles before starting
    /// let tempo: Tempo = handles.get("clock.tempo").expect("no tempo");
    ///
    /// // Start audio
    /// let running = runtime.start()?;
    ///
    /// // Control while running
    /// tempo.set_bpm(140.0);
    /// ```
    pub fn build(
        mut self,
        invention: Invention,
    ) -> Result<(InventionRuntime, InventionHandles), Box<dyn std::error::Error>> {
        let invention = self.resolve_assets(invention)?;
        self.register_developments(&invention)?;
        self.validate_invention(&invention)?;

        // Build all module instances (including sinks)
        let (modules, sinks, control_surfaces, handles) = self.build_modules(&invention)?;

        // Warn if no sink modules (invention will run but produce silence)
        if sinks.is_empty() && invention.outputs.is_empty() {
            eprintln!(
                "Warning: Invention '{}' has no sink modules or outputs. Audio output will be silent.",
                invention.title.as_deref().unwrap_or("untitled")
            );
        }

        // Build the routing graph
        let routing = self.build_routing(&invention, &modules)?;

        let state = Arc::new(Mutex::new(self.build_runtime_state(&invention)));

        let runtime = InventionRuntime {
            modules,
            sinks,
            control_surfaces,
            routing,
            registry: self.registry,
            sample_rate: self.sample_rate,
            state,
        };

        Ok((runtime, handles))
    }

    fn resolve_assets(
        &self,
        mut invention: Invention,
    ) -> Result<Invention, Box<dyn std::error::Error>> {
        if invention.assets.is_empty() {
            return Ok(invention);
        }

        let mut assets = HashMap::new();
        for (name, asset) in &invention.assets {
            let resolved = resolve_asset_path(invention.source_path.as_deref(), &asset.path)?;
            let contents = std::fs::read_to_string(&resolved).map_err(|err| {
                format!(
                    "Failed to read asset '{}' from '{}': {}",
                    name,
                    resolved.display(),
                    err
                )
            })?;
            let is_json = resolved
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
            let value = if is_json {
                serde_json::from_str(&contents).map_err(|err| {
                    format!(
                        "Failed to parse asset '{}' from '{}': {}",
                        name,
                        resolved.display(),
                        err
                    )
                })?
            } else {
                serde_json::Value::String(contents)
            };
            // Assets that declare themselves as scores opt into load-time
            // validation against `fugue.score.v1`. Other JSON assets are left
            // as-is, since assets are a general-purpose mechanism.
            if declares_score_schema(&value) {
                crate::invention::score::validate_score(&value).map_err(|err| {
                    format!(
                        "Invalid score asset '{}' from '{}': {}",
                        name,
                        resolved.display(),
                        err
                    )
                })?;
            }
            assets.insert(name.clone(), value);
        }

        for module in &mut invention.modules {
            module.config = resolve_asset_refs(&module.config, &assets)?;
        }

        Ok(invention)
    }

    fn build_runtime_state(&self, invention: &Invention) -> RuntimeState {
        let mut modules = IndexMap::new();
        for spec in &invention.modules {
            modules.insert(
                spec.id.clone(),
                RuntimeModuleInfo {
                    id: spec.id.clone(),
                    module_type: spec.module_type.clone(),
                    config: spec.config.clone(),
                },
            );
        }

        let connections = invention
            .connections
            .iter()
            .filter_map(|conn| {
                Some(RuntimeConnectionInfo {
                    from: conn.from.clone(),
                    from_port: conn.from_port.clone()?,
                    to: conn.to.clone(),
                    to_port: conn.to_port.clone()?,
                })
            })
            .collect();

        RuntimeState {
            modules,
            connections,
            sample_rate: self.sample_rate,
            running: false,
        }
    }

    fn validate_invention(&self, invention: &Invention) -> Result<(), Box<dyn std::error::Error>> {
        // Check all connections reference valid modules
        let module_ids: std::collections::HashSet<String> =
            invention.modules.iter().map(|m| m.id.clone()).collect();

        for conn in &invention.connections {
            if !module_ids.contains(&conn.from) {
                return Err(format!("Unknown source module: {}", conn.from).into());
            }
            if !module_ids.contains(&conn.to) {
                return Err(format!("Unknown destination module: {}", conn.to).into());
            }

            // Port names are required in modular system
            if conn.from_port.is_none() {
                return Err(format!("Missing from_port in connection from {}", conn.from).into());
            }
            if conn.to_port.is_none() {
                return Err(format!("Missing to_port in connection to {}", conn.to).into());
            }
        }

        Ok(())
    }

    fn build_modules(
        &self,
        invention: &Invention,
    ) -> Result<BuildModulesResult, Box<dyn std::error::Error>> {
        let mut modules = IndexMap::new();
        let mut sinks = Vec::new();
        let mut control_surfaces = IndexMap::new();
        let mut all_handles: HashMap<String, Arc<dyn Any + Send + Sync>> = HashMap::new();

        for spec in &invention.modules {
            // Build module via factory
            let result = self
                .registry
                .build(&spec.module_type, self.sample_rate, &spec.config)?;

            // If this is a sink, track its module id for output collection.
            if matches!(result.module, GraphModule::Sink(_)) {
                sinks.push(spec.id.clone());
            }

            // Store in modules collection
            modules.insert(spec.id.clone(), result.module);

            if let Some(control_surface) = result.control_surface {
                control_surfaces.insert(spec.id.clone(), control_surface);
            }

            // Collect handles with flat keys: "module_id.handle_name"
            for (handle_name, handle) in result.handles {
                let key = format!("{}.{}", spec.id, handle_name);
                all_handles.insert(key, handle);
            }
        }

        Ok((
            modules,
            sinks,
            control_surfaces,
            InventionHandles::new(all_handles),
        ))
    }

    fn build_routing(
        &self,
        invention: &Invention,
        modules: &IndexMap<String, ModuleInstance>,
    ) -> Result<Vec<RoutingConnection>, Box<dyn std::error::Error>> {
        let mut routing = Vec::new();

        for conn in &invention.connections {
            let from_port = conn.from_port.as_ref().ok_or("Missing from_port")?.clone();
            let to_port = conn.to_port.as_ref().ok_or("Missing to_port")?.clone();

            // Validate ports exist on modules
            if let Some(module) = modules.get(&conn.from) {
                validate_output_port(module, &from_port)?;
            }
            if let Some(module) = modules.get(&conn.to) {
                validate_input_port(module, &to_port)?;
            }

            routing.push(RoutingConnection {
                from_module: conn.from.clone(),
                from_port,
                to_module: conn.to.clone(),
                to_port,
            });
        }

        Ok(routing)
    }

    fn register_developments(
        &mut self,
        invention: &Invention,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for development in &invention.developments {
            {
                let mut registered = self.registered.lock().unwrap();
                if registered.contains(&development.name) {
                    continue;
                }
                registered.insert(development.name.clone());
            }

            let definition = self.load_development_definition(invention, development)?;
            let factory = DevelopmentFactory {
                name: development.name.clone(),
                definition,
                registry: self.registry.clone(),
                registered: self.registered.clone(),
            };
            self.registry.register_boxed(
                development.name.clone(),
                Arc::new(factory) as Arc<dyn ModuleFactory>,
            );
        }

        Ok(())
    }

    fn load_development_definition(
        &self,
        invention: &Invention,
        development: &super::format::DevelopmentSpec,
    ) -> Result<Invention, Box<dyn std::error::Error>> {
        match (&development.path, &development.definition) {
            (Some(path), None) => {
                let resolved = resolve_development_path(invention.source_path.as_deref(), path)?;
                Invention::from_file(&resolved.to_string_lossy())
            }
            (None, Some(definition)) => Ok((**definition).clone()),
            (Some(_), Some(_)) => Err(format!(
                "Development '{}' must set either 'path' or 'definition', not both",
                development.name
            )
            .into()),
            (None, None) => Err(format!(
                "Development '{}' must set either 'path' or 'definition'",
                development.name
            )
            .into()),
        }
    }
}

fn resolve_development_path(
    source_path: Option<&Path>,
    path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return Ok(candidate);
    }

    let Some(source_path) = source_path else {
        return Err(format!(
            "Relative development path '{}' requires the parent invention to be loaded from a file",
            path
        )
        .into());
    };

    let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(base_dir.join(candidate))
}

/// Returns true when a parsed JSON asset declares itself a `fugue.score.v1`
/// document via a top-level `schema` field, opting into load-time validation.
fn declares_score_schema(value: &serde_json::Value) -> bool {
    value
        .get("schema")
        .and_then(|schema| schema.as_str())
        .map(|schema| schema == crate::invention::score::SCORE_SCHEMA_V1)
        .unwrap_or(false)
}

fn resolve_asset_path(
    source_path: Option<&Path>,
    path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return Ok(candidate);
    }

    let Some(source_path) = source_path else {
        return Err(format!(
            "Relative asset path '{}' requires the parent invention to be loaded from a file",
            path
        )
        .into());
    };

    let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(base_dir.join(candidate))
}

fn resolve_asset_refs(
    value: &serde_json::Value,
    assets: &HashMap<String, serde_json::Value>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| resolve_asset_refs(item, assets))
            .collect(),
        serde_json::Value::Object(map) => {
            if let Some(asset_name) = map.get("$asset") {
                let asset_name = asset_name.as_str().ok_or("$asset must be a string")?;
                if map.keys().any(|key| key != "$asset" && key != "path") {
                    return Err(
                        "asset reference objects may only contain '$asset' and 'path'".into(),
                    );
                }

                let asset = assets
                    .get(asset_name)
                    .ok_or_else(|| format!("Unknown asset reference '{}'", asset_name))?;

                let Some(path) = map.get("path") else {
                    return Ok(asset.clone());
                };
                let path = path
                    .as_str()
                    .ok_or("asset reference path must be a string")?;
                if !path.is_empty() && !path.starts_with('/') {
                    return Err(format!(
                        "asset reference path '{}' must be a JSON Pointer starting with '/'",
                        path
                    )
                    .into());
                }

                asset
                    .pointer(path)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "Asset reference '{}' does not contain JSON Pointer '{}'",
                            asset_name, path
                        )
                    })
                    .map_err(Into::into)
            } else {
                map.iter()
                    .map(|(key, value)| Ok((key.clone(), resolve_asset_refs(value, assets)?)))
                    .collect::<Result<serde_json::Map<String, serde_json::Value>, Box<dyn std::error::Error>>>()
                    .map(serde_json::Value::Object)
            }
        }
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests;
