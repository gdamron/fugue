//! Type and instance discovery against a running invention's registry.

use super::RunningInvention;

impl RunningInvention {
    /// Discovers types in the current loaded registry, including registered
    /// developments. Each call observes the most recent successful reload.
    pub fn describe_module_types(
        &self,
        query: &crate::ModuleTypeQuery,
    ) -> Result<crate::ModuleTypeList, crate::RpcError> {
        crate::ModuleTypeList::from_registry(
            &self.registry(),
            self.sample_rate,
            crate::RegistryScope::Running,
            query,
        )
    }

    /// Inspects a type/config or reads an existing instance without rebuilding it.
    pub fn describe_module(
        &self,
        query: &crate::DescribeModuleQuery,
    ) -> Result<crate::ModuleDescription, crate::RpcError> {
        query.validate()?;
        if let Some(id) = &query.module_id {
            let ports = self.module_ports.lock().unwrap();
            let snapshot = self.snapshot().module_snapshot_with_ports(id, &ports)?;
            let is_sink = self.registry().is_sink(&snapshot.info.module_type);
            crate::ModuleDescription::from_snapshot(snapshot, self.sample_rate, is_sink)
        } else {
            crate::ModuleDescription::from_registry(
                &self.registry(),
                self.sample_rate,
                crate::RegistryScope::Running,
                query,
            )
        }
    }

    /// Adopts the development definitions a reload's validation build
    /// loaded, for the next reload to compare against. The reload's registry
    /// is adopted by its commit, under the publisher (see
    /// [`crate::invention::publish::LiveGraph::commit_adopting`]).
    pub(crate) fn adopt_definitions(
        &mut self,
        definitions: crate::invention::reload::DevelopmentDefinitions,
    ) {
        self.development_definitions = definitions;
    }
}
