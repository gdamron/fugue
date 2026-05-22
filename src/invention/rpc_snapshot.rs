use crate::rpc::{
    RuntimeControlSnapshot, RuntimeFullSnapshot, RuntimeModuleSnapshot, RuntimePortInfo,
};
use crate::{ControlSurface, RuntimeSnapshot};
use indexmap::IndexMap;

use super::orchestration::ModulePorts;

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
