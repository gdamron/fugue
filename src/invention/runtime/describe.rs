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
            // Read under the publisher before taking the port mirror, which
            // the publisher takes as it catches up.
            let timeline = self.live.timeline_of(id);
            let name = self.live.module_name(id);
            let ports = self.module_ports.lock().unwrap();
            let mut snapshot = self.snapshot().module_snapshot_with_ports(id, &ports)?;
            snapshot.timeline = timeline;
            let mut description = crate::ModuleDescription::from_snapshot(
                snapshot,
                self.sample_rate,
                &self.registry(),
            )?;
            description.name = name;
            Ok(description)
        } else {
            crate::ModuleDescription::from_registry(
                &self.registry(),
                self.sample_rate,
                crate::RegistryScope::Running,
                query,
            )
        }
    }

    /// The development `path` names (see [`crate::RpcCommand::GetDevelopment`]),
    /// as the running piece loaded it, at `revision`. Read on the calling
    /// control thread from the definitions kept at load and reload; nothing
    /// the audio thread reads is touched, and no file is read again.
    pub fn development(
        &self,
        path: &[String],
        revision: crate::rpc::RuntimeRevision,
    ) -> Result<crate::rpc::DevelopmentSnapshot, crate::RpcError> {
        let definitions = self.definitions();
        let (declared, definition) = definitions.development(path)?;
        let saved_in = definitions.saved_in(path)?;
        crate::rpc::DevelopmentSnapshot::bounded(revision, path, declared, saved_in, definition)
    }

    /// The development definitions the running piece holds (see
    /// [`RuntimeState::developments`](crate::invention::state::RuntimeState)),
    /// shared, not copied.
    pub(crate) fn definitions(&self) -> crate::invention::reload::DevelopmentDefinitions {
        self.state.lock().unwrap().developments.clone()
    }
}
