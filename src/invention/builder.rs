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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ControlValue, DevelopmentControl, DevelopmentInput, DevelopmentOutput, DevelopmentSpec,
    };

    fn voice_development() -> Invention {
        Invention {
            version: "1.0.0".to_string(),
            title: Some("voice".to_string()),
            description: None,
            developments: vec![],
            modules: vec![
                crate::ModuleSpec {
                    id: "osc".to_string(),
                    module_type: "oscillator".to_string(),
                    config: serde_json::json!({"frequency": 220.0}),
                },
                crate::ModuleSpec {
                    id: "vca".to_string(),
                    module_type: "vca".to_string(),
                    config: serde_json::json!({"level": 1.0}),
                },
            ],
            connections: vec![crate::Connection {
                from: "osc".to_string(),
                to: "vca".to_string(),
                from_port: Some("audio".to_string()),
                to_port: Some("audio".to_string()),
            }],
            inputs: vec![DevelopmentInput {
                name: "frequency".to_string(),
                to: "osc".to_string(),
                to_port: "frequency".to_string(),
            }],
            outputs: vec![DevelopmentOutput {
                name: "audio".to_string(),
                from: "vca".to_string(),
                from_port: "audio".to_string(),
            }],
            controls: vec![DevelopmentControl {
                key: "type".to_string(),
                module: "osc".to_string(),
                control: "type".to_string(),
            }],
            source_path: None,
        }
    }

    fn root_invention_with_voice(voice: DevelopmentSpec) -> Invention {
        Invention {
            version: "1.0.0".to_string(),
            title: Some("root".to_string()),
            description: None,
            developments: vec![voice],
            modules: vec![
                crate::ModuleSpec {
                    id: "lead".to_string(),
                    module_type: "voice".to_string(),
                    config: serde_json::Value::Null,
                },
                crate::ModuleSpec {
                    id: "dac".to_string(),
                    module_type: "dac".to_string(),
                    config: serde_json::Value::Null,
                },
            ],
            connections: vec![crate::Connection {
                from: "lead".to_string(),
                to: "dac".to_string(),
                from_port: Some("audio".to_string()),
                to_port: Some("audio".to_string()),
            }],
            inputs: vec![],
            outputs: vec![],
            controls: vec![],
            source_path: None,
        }
    }

    #[test]
    fn builds_inline_development_as_module() {
        let invention = root_invention_with_voice(DevelopmentSpec {
            name: "voice".to_string(),
            path: None,
            definition: Some(Box::new(voice_development())),
        });

        let builder = InventionBuilder::new(44_100);
        let (runtime, _) = builder.build(invention).unwrap();

        let module = runtime.modules.get("lead").unwrap().module();
        assert!(module.inputs().contains(&"frequency"));
        assert!(module.outputs().contains(&"audio"));

        let controls = runtime.control_surfaces.get("lead").unwrap().controls();
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].key, "type");
    }

    #[test]
    fn development_controls_alias_internal_surface() {
        let invention = root_invention_with_voice(DevelopmentSpec {
            name: "voice".to_string(),
            path: None,
            definition: Some(Box::new(voice_development())),
        });

        let builder = InventionBuilder::new(44_100);
        let (runtime, _) = builder.build(invention).unwrap();
        let surface = runtime.control_surfaces.get("lead").unwrap();

        surface
            .set_control("type", ControlValue::String("square".to_string()))
            .unwrap();
        assert_eq!(
            surface.get_control("type").unwrap(),
            ControlValue::String("square".to_string())
        );
    }

    #[test]
    fn development_fans_out_exposed_inputs_and_caches_outputs() {
        let development = Invention {
            version: "1.0.0".to_string(),
            title: Some("fanout".to_string()),
            description: None,
            developments: vec![],
            modules: vec![
                crate::ModuleSpec {
                    id: "full".to_string(),
                    module_type: "vca".to_string(),
                    config: serde_json::json!({"cv": 1.0}),
                },
                crate::ModuleSpec {
                    id: "half".to_string(),
                    module_type: "vca".to_string(),
                    config: serde_json::json!({"cv": 0.5}),
                },
            ],
            connections: vec![],
            inputs: vec![
                DevelopmentInput {
                    name: "signal".to_string(),
                    to: "full".to_string(),
                    to_port: "audio".to_string(),
                },
                DevelopmentInput {
                    name: "signal".to_string(),
                    to: "half".to_string(),
                    to_port: "audio".to_string(),
                },
            ],
            outputs: vec![
                DevelopmentOutput {
                    name: "full".to_string(),
                    from: "full".to_string(),
                    from_port: "audio".to_string(),
                },
                DevelopmentOutput {
                    name: "half".to_string(),
                    from: "half".to_string(),
                    from_port: "audio".to_string(),
                },
            ],
            controls: vec![],
            source_path: None,
        };

        let root = Invention {
            version: "1.0.0".to_string(),
            title: Some("root".to_string()),
            description: None,
            developments: vec![DevelopmentSpec {
                name: "fanout".to_string(),
                path: None,
                definition: Some(Box::new(development)),
            }],
            modules: vec![crate::ModuleSpec {
                id: "voice".to_string(),
                module_type: "fanout".to_string(),
                config: serde_json::Value::Null,
            }],
            connections: vec![],
            inputs: vec![],
            outputs: vec![],
            controls: vec![],
            source_path: None,
        };

        let (mut runtime, _) = InventionBuilder::new(44_100).build(root).unwrap();
        let voice = runtime.modules.get_mut("voice").unwrap().module_mut();

        voice.set_input("signal", 0.8).unwrap();
        voice.process();

        assert_eq!(voice.get_output("full").unwrap(), 0.8);
        assert_eq!(voice.get_output("half").unwrap(), 0.4);
    }

    #[test]
    fn resolves_relative_development_paths() {
        let unique = format!(
            "fugue-development-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).unwrap();

        let development_path = dir.join("voice.json");
        std::fs::write(&development_path, voice_development().to_json().unwrap()).unwrap();

        let root = root_invention_with_voice(DevelopmentSpec {
            name: "voice".to_string(),
            path: Some("voice.json".to_string()),
            definition: None,
        });
        let root_path = dir.join("root.json");
        std::fs::write(&root_path, root.to_json().unwrap()).unwrap();

        let invention = Invention::from_file(&root_path.to_string_lossy()).unwrap();
        let builder = InventionBuilder::new(44_100);
        let (runtime, _) = builder.build(invention).unwrap();

        assert!(runtime.modules.contains_key("lead"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn supports_nested_developments() {
        let inner = Invention {
            version: "1.0.0".to_string(),
            title: Some("inner".to_string()),
            description: None,
            developments: vec![],
            modules: vec![crate::ModuleSpec {
                id: "osc".to_string(),
                module_type: "oscillator".to_string(),
                config: serde_json::json!({"frequency": 440.0}),
            }],
            connections: vec![],
            inputs: vec![],
            outputs: vec![DevelopmentOutput {
                name: "audio".to_string(),
                from: "osc".to_string(),
                from_port: "audio".to_string(),
            }],
            controls: vec![],
            source_path: None,
        };
        let outer = Invention {
            version: "1.0.0".to_string(),
            title: Some("outer".to_string()),
            description: None,
            developments: vec![DevelopmentSpec {
                name: "inner_voice".to_string(),
                path: None,
                definition: Some(Box::new(inner)),
            }],
            modules: vec![crate::ModuleSpec {
                id: "voice".to_string(),
                module_type: "inner_voice".to_string(),
                config: serde_json::Value::Null,
            }],
            connections: vec![],
            inputs: vec![],
            outputs: vec![DevelopmentOutput {
                name: "audio".to_string(),
                from: "voice".to_string(),
                from_port: "audio".to_string(),
            }],
            controls: vec![],
            source_path: None,
        };
        let root = root_invention_with_voice(DevelopmentSpec {
            name: "voice".to_string(),
            path: None,
            definition: Some(Box::new(outer)),
        });

        let builder = InventionBuilder::new(44_100);
        let (runtime, _) = builder.build(root).unwrap();

        assert!(runtime.modules.contains_key("lead"));
    }
}
