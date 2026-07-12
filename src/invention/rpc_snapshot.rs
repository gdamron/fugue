use crate::rpc::{
    RuntimeControlSnapshot, RuntimeFullSnapshot, RuntimeModuleSnapshot, RuntimePortInfo,
};
use crate::{Connection, ControlSurface, ControlValue, Invention, ModuleSpec, RuntimeSnapshot};
use indexmap::IndexMap;

use super::orchestration::ModulePorts;

/// A single control value to re-apply after rebuilding an invention from a snapshot.
///
/// Tuple of `(module_id, control_key, value)`.
pub type ControlOverride = (String, String, ControlValue);

impl RuntimeFullSnapshot {
    /// Reconstructs a declarative [`Invention`] describing the snapshot's topology.
    ///
    /// Modules are rebuilt from their stored `module_type` and original `config`
    /// payload, and connections are mapped back to the file-format
    /// [`Connection`] shape (empty port strings become `None`). The result is a
    /// cold-buildable graph: it does **not** carry current control values, which
    /// are returned separately by [`RuntimeFullSnapshot::control_overrides`] so
    /// they can be re-applied after the rebuilt invention starts.
    pub fn to_invention(&self) -> Invention {
        let modules = self
            .modules
            .iter()
            .map(|module| ModuleSpec {
                id: module.info.id.clone(),
                module_type: module.info.module_type.clone(),
                config: module.info.config.clone(),
            })
            .collect();

        let connections = self
            .connections
            .iter()
            .map(|connection| Connection {
                from: connection.from.clone(),
                to: connection.to.clone(),
                from_port: optional_port(&connection.from_port),
                to_port: optional_port(&connection.to_port),
            })
            .collect();

        Invention {
            version: "1.0.0".to_string(),
            title: None,
            description: None,
            developments: Vec::new(),
            assets: std::collections::BTreeMap::new(),
            modules,
            connections,
            inputs: Vec::new(),
            outputs: Vec::new(),
            controls: Vec::new(),
            source_path: None,
        }
    }

    /// Collects the current control values to re-apply after a rebuild.
    ///
    /// Only controls whose value is known (`Some`) are included; modules expose
    /// their live value via [`ControlSurface::get_control`], so this captures any
    /// runtime mutations that diverged from the build-time config.
    pub fn control_overrides(&self) -> Vec<ControlOverride> {
        self.modules
            .iter()
            .flat_map(|module| {
                let module_id = module.info.id.clone();
                module.controls.iter().filter_map(move |control| {
                    control
                        .value
                        .clone()
                        .map(|value| (module_id.clone(), control.meta.key.clone(), value))
                })
            })
            .collect()
    }
}

/// Maps a runtime port string to the file-format optional port: empty means
/// "default/unnamed port", which the format represents as `None`.
fn optional_port(port: &str) -> Option<String> {
    if port.is_empty() {
        None
    } else {
        Some(port.to_string())
    }
}

impl RuntimeSnapshot {
    /// Builds a serializable RPC snapshot of topology, controls, and status.
    pub fn full_snapshot(&self) -> RuntimeFullSnapshot {
        self.full_snapshot_with_ports(&IndexMap::new())
    }

    pub(crate) fn full_snapshot_with_ports(
        &self,
        module_ports: &IndexMap<String, ModulePorts>,
    ) -> RuntimeFullSnapshot {
        let state = self.state.lock().unwrap();
        let status = state.status();
        let module_infos: Vec<_> = state.modules.values().cloned().collect();
        let connections = state.connections.clone();
        drop(state);

        let controls = self.control_surfaces.lock().unwrap();
        let modules = module_infos
            .into_iter()
            .map(|info| {
                let control_snapshots = controls
                    .get(&info.id)
                    .map(|surface| snapshot_controls(surface.as_ref()))
                    .unwrap_or_default();

                RuntimeModuleSnapshot {
                    ports: module_ports
                        .get(&info.id)
                        .map(|ports| RuntimePortInfo {
                            inputs: ports.inputs.clone(),
                            outputs: ports.outputs.clone(),
                        })
                        .unwrap_or_default(),
                    info,
                    controls: control_snapshots,
                }
            })
            .collect();

        RuntimeFullSnapshot {
            status,
            modules,
            connections,
        }
    }
}

fn snapshot_controls(surface: &(dyn ControlSurface + Send + Sync)) -> Vec<RuntimeControlSnapshot> {
    surface
        .controls()
        .into_iter()
        .map(|meta| {
            let value = surface.get_control(&meta.key).ok();
            RuntimeControlSnapshot { meta, value }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{RuntimeModuleSnapshot, RuntimePortInfo};
    use crate::{
        ControlMeta, RenderEngine, RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeStatus,
    };

    fn module(id: &str, module_type: &str, config: serde_json::Value) -> RuntimeModuleSnapshot {
        RuntimeModuleSnapshot {
            info: RuntimeModuleInfo {
                id: id.to_string(),
                module_type: module_type.to_string(),
                config,
            },
            ports: RuntimePortInfo::default(),
            controls: Vec::new(),
        }
    }

    fn empty_status() -> RuntimeStatus {
        RuntimeStatus {
            running: false,
            sample_rate: 48_000,
            module_count: 0,
            connection_count: 0,
            diagnostics: None,
        }
    }

    #[test]
    fn to_invention_maps_modules_and_ports() {
        let snapshot = RuntimeFullSnapshot {
            status: empty_status(),
            modules: vec![
                module(
                    "osc",
                    "oscillator",
                    serde_json::json!({ "frequency": 440.0 }),
                ),
                module("dac", "dac", serde_json::Value::Null),
            ],
            connections: vec![
                // Named ports survive.
                RuntimeConnectionInfo {
                    from: "osc".into(),
                    from_port: "audio".into(),
                    to: "dac".into(),
                    to_port: "audio".into(),
                },
                // Empty port strings collapse to None.
                RuntimeConnectionInfo {
                    from: "osc".into(),
                    from_port: String::new(),
                    to: "dac".into(),
                    to_port: String::new(),
                },
            ],
        };

        let invention = snapshot.to_invention();

        assert_eq!(invention.modules.len(), 2);
        assert_eq!(invention.modules[0].id, "osc");
        assert_eq!(invention.modules[0].module_type, "oscillator");
        assert_eq!(invention.modules[0].config["frequency"], 440.0);

        assert_eq!(invention.connections.len(), 2);
        assert_eq!(invention.connections[0].from_port.as_deref(), Some("audio"));
        assert_eq!(invention.connections[0].to_port.as_deref(), Some("audio"));
        assert_eq!(invention.connections[1].from_port, None);
        assert_eq!(invention.connections[1].to_port, None);
    }

    #[test]
    fn control_overrides_skips_unknown_values() {
        let mut osc = module("osc", "oscillator", serde_json::Value::Null);
        osc.controls = vec![
            RuntimeControlSnapshot {
                meta: ControlMeta::number("frequency", "oscillator frequency"),
                value: Some(ControlValue::Number(220.0)),
            },
            RuntimeControlSnapshot {
                meta: ControlMeta::number("phase", "oscillator phase"),
                value: None,
            },
        ];

        let snapshot = RuntimeFullSnapshot {
            status: empty_status(),
            modules: vec![osc],
            connections: Vec::new(),
        };

        let overrides = snapshot.control_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].0, "osc");
        assert_eq!(overrides[0].1, "frequency");
        assert_eq!(overrides[0].2, ControlValue::Number(220.0));
    }

    #[test]
    fn round_trips_a_built_invention_through_render_engine() {
        const INVENTION: &str = r#"{
            "version": "1.0.0",
            "title": "round-trip",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
                { "id": "vca", "type": "vca", "config": { "level": 0.0 } },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
                { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#;

        let mut engine = RenderEngine::new(48_000);
        engine.load_json(INVENTION).unwrap();
        // Diverge a control from its build-time config value.
        engine
            .set_control("vca", "cv", ControlValue::Number(0.5))
            .unwrap();

        let snapshot = engine.full_snapshot();
        let invention = snapshot.to_invention();

        // Topology reconstructs: rebuilding the derived invention succeeds and
        // yields the same module/connection counts.
        let mut rebuilt = RenderEngine::new(48_000);
        rebuilt
            .load_json(&invention.to_json().unwrap())
            .expect("snapshot-derived invention rebuilds");
        let rebuilt_snapshot = rebuilt.full_snapshot();
        assert_eq!(rebuilt_snapshot.modules.len(), snapshot.modules.len());
        assert_eq!(
            rebuilt_snapshot.connections.len(),
            snapshot.connections.len()
        );

        // The runtime mutation is captured as an override and re-applies cleanly.
        let overrides = snapshot.control_overrides();
        assert!(overrides.iter().any(|(module, key, value)| module == "vca"
            && key == "cv"
            && *value == ControlValue::Number(0.5)));
        for (module, key, value) in overrides {
            rebuilt.set_control(&module, &key, value).unwrap();
        }
        assert_eq!(
            rebuilt.get_control("vca", "cv").unwrap(),
            ControlValue::Number(0.5)
        );
    }
}
