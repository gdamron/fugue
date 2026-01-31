//! Patch builder for creating modular synthesis setups.

use crate::patch::format::Patch;
use crate::patch::handles::PatchHandles;
use crate::patch::runtime::{
    validate_input_port, validate_output_port, ModuleInstance, PatchRuntime,
};
use crate::registry::ModuleRegistry;
use indexmap::IndexMap;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use super::graph::{RoutingConnection, SinkInstance};

/// Result type for building modules, containing modules, sinks, and handles.
type BuildModulesResult = (
    IndexMap<String, ModuleInstance>,
    IndexMap<String, SinkInstance>,
    PatchHandles,
);

/// Patch builder that uses named port routing.
///
/// Modules are connected via explicit port names rather than type-based routing.
/// The builder uses a registry to construct modules from their type names.
pub struct PatchBuilder {
    sample_rate: u32,
    registry: ModuleRegistry,
}

impl PatchBuilder {
    /// Creates a new patch builder with the default module registry.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            registry: ModuleRegistry::default(),
        }
    }

    /// Creates a new patch builder with a custom module registry.
    pub fn with_registry(sample_rate: u32, registry: ModuleRegistry) -> Self {
        Self {
            sample_rate,
            registry,
        }
    }

    /// Builds and prepares a patch for execution.
    ///
    /// Returns both the runtime (for starting audio) and handles (for runtime control).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let patch = Patch::from_file("my_patch.json")?;
    /// let builder = PatchBuilder::new(44100);
    /// let (runtime, handles) = builder.build(patch)?;
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
        &self,
        patch: Patch,
    ) -> Result<(PatchRuntime, PatchHandles), Box<dyn std::error::Error>> {
        self.validate_patch(&patch)?;

        // Build all module instances (including sinks)
        let (modules, sinks, handles) = self.build_modules(&patch)?;

        // Warn if no sink modules (patch will run but produce silence)
        if sinks.is_empty() {
            eprintln!(
                "Warning: Patch '{}' has no sink modules. Audio output will be silent.",
                patch.title.as_deref().unwrap_or("untitled")
            );
        }

        // Build the routing graph
        let routing = self.build_routing(&patch, &modules)?;

        let runtime = PatchRuntime {
            modules,
            sinks,
            routing,
        };

        Ok((runtime, handles))
    }

    fn validate_patch(&self, patch: &Patch) -> Result<(), Box<dyn std::error::Error>> {
        // Check all connections reference valid modules
        let module_ids: std::collections::HashSet<String> =
            patch.modules.iter().map(|m| m.id.clone()).collect();

        for conn in &patch.connections {
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

        // Check for cycles in the dependency graph
        self.validate_acyclic(patch)?;

        Ok(())
    }

    /// Validates that the patch contains no cycles (feedback loops).
    ///
    /// Uses depth-first search with a recursion stack to detect cycles.
    /// Cycles would cause infinite recursion in the pull-based system.
    fn validate_acyclic(&self, patch: &Patch) -> Result<(), Box<dyn std::error::Error>> {
        // Build adjacency list (module -> modules it connects to)
        // Exclude sink modules as destinations (they're terminals)
        let sink_types: std::collections::HashSet<&str> = patch
            .modules
            .iter()
            .filter(|m| self.registry.is_sink(&m.module_type))
            .map(|m| m.id.as_str())
            .collect();

        let mut graph: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for conn in &patch.connections {
            // Don't include connections TO sinks in cycle detection (they're terminals)
            if !sink_types.contains(conn.to.as_str()) {
                graph
                    .entry(conn.from.clone())
                    .or_default()
                    .push(conn.to.clone());
            }
        }

        // Check each module for cycles using DFS
        let mut visited = std::collections::HashSet::new();
        let mut rec_stack = std::collections::HashSet::new();

        for module in &patch.modules {
            // Skip sink modules in cycle detection (they're terminals)
            if sink_types.contains(module.id.as_str()) {
                continue;
            }

            if !visited.contains(&module.id)
                && Self::has_cycle_dfs(&module.id, &graph, &mut visited, &mut rec_stack)
            {
                return Err(format!(
                    "Cycle detected in signal graph involving module '{}'",
                    module.id
                )
                .into());
            }
        }

        Ok(())
    }

    /// Depth-first search to detect cycles.
    ///
    /// Returns true if a cycle is detected starting from `node`.
    fn has_cycle_dfs(
        node: &str,
        graph: &std::collections::HashMap<String, Vec<String>>,
        visited: &mut std::collections::HashSet<String>,
        rec_stack: &mut std::collections::HashSet<String>,
    ) -> bool {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());

        // Check all neighbors
        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if Self::has_cycle_dfs(neighbor, graph, visited, rec_stack) {
                        return true;
                    }
                } else if rec_stack.contains(neighbor) {
                    // Found a back edge - cycle detected!
                    return true;
                }
            }
        }

        rec_stack.remove(node);
        false
    }

    fn build_modules(
        &self,
        patch: &Patch,
    ) -> Result<BuildModulesResult, Box<dyn std::error::Error>> {
        let mut modules = IndexMap::new();
        let mut sinks = IndexMap::new();
        let mut all_handles: HashMap<String, Arc<dyn Any + Send + Sync>> = HashMap::new();

        for spec in &patch.modules {
            // Build module via factory
            let result = self
                .registry
                .build(&spec.module_type, self.sample_rate, &spec.config)?;

            // Store in modules collection
            modules.insert(spec.id.clone(), result.module);

            // If this is a sink, also store in sinks collection
            if let Some(sink) = result.sink {
                sinks.insert(spec.id.clone(), sink);
            }

            // Collect handles with flat keys: "module_id.handle_name"
            for (handle_name, handle) in result.handles {
                let key = format!("{}.{}", spec.id, handle_name);
                all_handles.insert(key, handle);
            }
        }

        Ok((modules, sinks, PatchHandles::new(all_handles)))
    }

    fn build_routing(
        &self,
        patch: &Patch,
        modules: &IndexMap<String, ModuleInstance>,
    ) -> Result<Vec<RoutingConnection>, Box<dyn std::error::Error>> {
        let mut routing = Vec::new();

        for conn in &patch.connections {
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
}
