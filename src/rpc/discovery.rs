//! Bounded discovery of registered module types and configured instances.

use super::{RpcError, RpcErrorCode, RuntimeModuleSnapshot};
use crate::{ControlMeta, ControlValue, ModuleRegistry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version of module discovery responses, independent of the transport envelope.
pub const MODULE_DISCOVERY_SCHEMA_VERSION: u32 = 1;
/// Maximum number of exact type names accepted in a discovery filter.
pub const MAX_DISCOVERY_TYPES: usize = 64;
/// Maximum serialized discovery payload size; callers must narrow oversized queries.
pub const MAX_DISCOVERY_RESPONSE_BYTES: usize = 65_536;

/// Registry used to answer a discovery request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RegistryScope {
    /// No invention is loaded; only built-in factories are available.
    Builtins,
    /// The loaded invention's registry, including its registered developments.
    Running,
}

/// Terse discovery response. Listing names never constructs module instances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ModuleTypeIndex {
    /// Version of this response's shape and semantics.
    pub schema_version: u32,
    /// Whether the names came from the built-in or loaded registry.
    pub registry_scope: RegistryScope,
    /// Unique registered names in lexicographic order.
    pub types: Vec<String>,
}

impl ModuleTypeIndex {
    /// Lists all registered names, or an exact filter. Empty filters are rejected;
    /// duplicates are deduplicated and unknown names fail the entire request.
    pub fn from_registry(
        registry: &ModuleRegistry,
        scope: RegistryScope,
        types: Option<&[String]>,
    ) -> Result<Self, RpcError> {
        let mut names = match types {
            Some(types) => {
                if types.is_empty() || types.len() > MAX_DISCOVERY_TYPES {
                    return Err(RpcError::new(
                        RpcErrorCode::InvalidRequest,
                        format!(
                            "types must contain between 1 and {MAX_DISCOVERY_TYPES} exact names"
                        ),
                    ));
                }
                for name in types {
                    validate_name(name)?;
                }
                types.to_vec()
            }
            None => registry.types().map(str::to_owned).collect(),
        };
        names.sort();
        names.dedup();
        for name in &names {
            if !registry.has_type(name) {
                return Err(RpcError::new(
                    RpcErrorCode::UnknownModuleType,
                    format!("unknown module type: {name}"),
                ));
            }
        }
        let index = Self {
            schema_version: MODULE_DISCOVERY_SCHEMA_VERSION,
            registry_scope: scope,
            types: names,
        };
        check_discovery_size(&index)?;
        Ok(index)
    }
}

/// Refuses an oversized discovery payload without returning a truncated success.
pub fn check_discovery_size(value: &impl Serialize) -> Result<(), RpcError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| RpcError::new(RpcErrorCode::Internal, error.to_string()))?;
    if bytes.len() > MAX_DISCOVERY_RESPONSE_BYTES {
        return Err(RpcError::new(
            RpcErrorCode::ResponseTooLarge,
            format!(
                "discovery response exceeds {MAX_DISCOVERY_RESPONSE_BYTES} bytes; narrow the types filter or inspect one module"
            ),
        ));
    }
    Ok(())
}

/// Requested catalog detail. Full details describe each type's default config.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TypeDetail {
    /// Names only, without constructing any modules.
    #[default]
    Index,
    /// Ports and controls, or an explicit per-type inspection error.
    Full,
}

/// Parameters shared by MCP and daemon type discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ModuleTypeQuery {
    /// Optional exact names (1–64); unknown names fail the request. Omit for all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    /// Defaults to index; request full explicitly for default-config metadata.
    #[serde(default)]
    pub detail: TypeDetail,
}

/// Parameters for one module inspection; exactly one of type or module_id is required.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DescribeModuleQuery {
    /// Registered type name. Omit when selecting a running module_id.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub module_type: Option<String>,
    /// Existing running instance ID; reads actual ports and controls without rebuilding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    /// Type-specific config with creation's defaults/validation. Omit or null for defaults.
    /// Not accepted with module_id. Maximum serialized size: 16 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

impl DescribeModuleQuery {
    /// Validates selectors and input bounds before any construction or lookup.
    pub fn validate(&self) -> Result<(), RpcError> {
        if self.module_type.is_some() == self.module_id.is_some()
            || (self.module_id.is_some() && self.config.is_some())
        {
            return Err(RpcError::new(
                RpcErrorCode::InvalidRequest,
                "pass either type (with optional config) or module_id, exclusively",
            ));
        }
        validate_name(
            self.module_type
                .as_ref()
                .or(self.module_id.as_ref())
                .unwrap(),
        )?;
        if let Some(config) = &self.config {
            if serde_json::to_vec(config)
                .map_err(|e| RpcError::new(RpcErrorCode::Internal, e.to_string()))?
                .len()
                > 16_384
            {
                return Err(RpcError::new(
                    RpcErrorCode::InvalidRequest,
                    "config exceeds 16384 bytes",
                ));
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), RpcError> {
    if name.is_empty() || name.len() > 256 {
        return Err(RpcError::new(
            RpcErrorCode::InvalidRequest,
            "type names and module IDs must contain 1–256 UTF-8 bytes",
        ));
    }
    Ok(())
}

/// Provenance of metadata; defaults never stand in for a configured instance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MetadataSource {
    /// No config was supplied; the factory's defaults were inspected.
    TypeDefaults,
    /// The supplied config was used for a temporary read-only instance.
    SuppliedConfig,
    /// Metadata and values were read from the existing running instance.
    RunningInstance,
}

/// Actual ports and control metadata from a successfully built module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ModuleTypeInfo {
    /// Registry name of the module type.
    pub type_name: String,
    /// Input ports in the module's declared order.
    pub inputs: Vec<String>,
    /// Output ports in the module's declared order.
    pub outputs: Vec<String>,
    /// Control descriptions and declared defaults, in key order.
    pub controls: Vec<ControlMeta>,
    /// Whether the registered factory produces a sink.
    pub is_sink: bool,
}

/// One full catalog entry. Failures carry no fabricated port/control arrays.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModuleTypeDetail {
    /// Default config could be inspected successfully.
    Available {
        #[serde(flatten)]
        info: ModuleTypeInfo,
    },
    /// Default config could not be inspected; try a supplied config or running instance.
    Unavailable { type_name: String, error: RpcError },
}

/// Bounded type catalog, with optional explicit full detail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ModuleTypeList {
    /// Compact names and registry provenance.
    #[serde(flatten)]
    pub index: ModuleTypeIndex,
    /// Present only for full detail: these are type defaults, not live instances.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_source: Option<MetadataSource>,
    /// Sample rate used for full inspection, omitted for a name-only index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// Per-type metadata or errors in the same order as types. Omitted for index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<ModuleTypeDetail>>,
}

impl ModuleTypeList {
    /// Queries the current registry without caching across reloads.
    pub fn from_registry(
        registry: &ModuleRegistry,
        sample_rate: u32,
        scope: RegistryScope,
        query: &ModuleTypeQuery,
    ) -> Result<Self, RpcError> {
        let index = ModuleTypeIndex::from_registry(registry, scope, query.types.as_deref())?;
        let full = query.detail == TypeDetail::Full;
        if full && index.types.len() > MAX_DISCOVERY_TYPES {
            return Err(RpcError::new(
                RpcErrorCode::ResponseTooLarge,
                "full discovery is limited to 64 types; narrow the types filter",
            ));
        }
        let mut response = Self {
            index,
            metadata_source: full.then_some(MetadataSource::TypeDefaults),
            sample_rate: full.then_some(sample_rate),
            details: full.then(Vec::new),
        };
        if full {
            for name in response.index.types.clone() {
                let entry = match ModuleDescription::from_registry(
                    registry,
                    sample_rate,
                    scope,
                    &DescribeModuleQuery {
                        module_type: Some(name.clone()),
                        ..Default::default()
                    },
                ) {
                    Ok(description) => ModuleTypeDetail::Available {
                        info: description.info,
                    },
                    Err(error) => ModuleTypeDetail::Unavailable {
                        type_name: name,
                        error,
                    },
                };
                response.details.as_mut().unwrap().push(entry);
                check_discovery_size(&response)?;
            }
        }
        check_discovery_size(&response)?;
        Ok(response)
    }
}

/// Detailed inspection of one module, including initial or current control values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ModuleDescription {
    /// Discovery response schema version.
    pub schema_version: u32,
    /// Registry used for the lookup.
    pub registry_scope: RegistryScope,
    /// Type defaults, supplied config, or an existing running instance.
    pub metadata_source: MetadataSource,
    /// Sample rate used by the inspected instance.
    pub sample_rate: u32,
    /// Present only for a running instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    /// Ports and declared control metadata.
    #[serde(flatten)]
    pub info: ModuleTypeInfo,
    /// Initial values for supplied config/defaults; current values for a running instance.
    pub control_values: BTreeMap<String, ControlValue>,
}

impl ModuleDescription {
    /// Inspects a supplied config or defaults without activating output backends.
    /// Factory validation/defaults apply; external output readiness is not tested.
    pub fn from_registry(
        registry: &ModuleRegistry,
        sample_rate: u32,
        scope: RegistryScope,
        query: &DescribeModuleQuery,
    ) -> Result<Self, RpcError> {
        query.validate()?;
        let name = query.module_type.as_ref().ok_or_else(|| {
            RpcError::new(
                RpcErrorCode::UnknownModule,
                "no running invention; module_id inspection requires a loaded invention",
            )
        })?;
        if !registry.has_type(name) {
            return Err(RpcError::new(
                RpcErrorCode::UnknownModuleType,
                format!("unknown module type: {name}"),
            ));
        }
        let config = query.config.as_ref().unwrap_or(&serde_json::Value::Null);
        let built = registry
            .for_inspection()
            .build(name, sample_rate, config)
            .map_err(|error| {
                let message = error.to_string();
                if message.len() > MAX_DISCOVERY_RESPONSE_BYTES / 2 {
                    RpcError::new(
                        RpcErrorCode::ResponseTooLarge,
                        "module inspection error exceeds response limit",
                    )
                } else {
                    RpcError::new(RpcErrorCode::ModuleBuildFailed, message)
                }
            })?;
        let module = built.module.module();
        let mut controls = built
            .control_surface
            .as_ref()
            .map(|s| s.controls())
            .unwrap_or_default();
        controls.sort_by(|a, b| a.key.cmp(&b.key));
        let mut control_values = BTreeMap::new();
        if let Some(surface) = &built.control_surface {
            for control in &controls {
                if let Ok(value) = surface.get_control(&control.key) {
                    control_values.insert(control.key.clone(), value);
                }
            }
        }
        let response = Self {
            schema_version: MODULE_DISCOVERY_SCHEMA_VERSION,
            registry_scope: scope,
            metadata_source: if config.is_null() {
                MetadataSource::TypeDefaults
            } else {
                MetadataSource::SuppliedConfig
            },
            sample_rate,
            module_id: None,
            info: ModuleTypeInfo {
                type_name: name.clone(),
                inputs: module.inputs().iter().map(|p| p.to_string()).collect(),
                outputs: module.outputs().iter().map(|p| p.to_string()).collect(),
                controls,
                is_sink: registry.is_sink(name),
            },
            control_values,
        };
        check_discovery_size(&response)?;
        Ok(response)
    }

    /// Uses the captured live instance metadata, without rebuilding its type.
    pub(crate) fn from_snapshot(
        snapshot: RuntimeModuleSnapshot,
        sample_rate: u32,
        is_sink: bool,
    ) -> Result<Self, RpcError> {
        let control_values = snapshot
            .controls
            .iter()
            .filter_map(|c| c.value.clone().map(|value| (c.meta.key.clone(), value)))
            .collect();
        let mut controls: Vec<_> = snapshot.controls.into_iter().map(|c| c.meta).collect();
        controls.sort_by(|a, b| a.key.cmp(&b.key));
        let response = Self {
            schema_version: MODULE_DISCOVERY_SCHEMA_VERSION,
            registry_scope: RegistryScope::Running,
            metadata_source: MetadataSource::RunningInstance,
            sample_rate,
            module_id: Some(snapshot.info.id),
            info: ModuleTypeInfo {
                type_name: snapshot.info.module_type,
                inputs: snapshot.ports.inputs,
                outputs: snapshot.ports.outputs,
                controls,
                is_sink,
            },
            control_values,
        };
        check_discovery_size(&response)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_does_not_construct_factories() {
        struct Unbuildable;
        impl crate::ModuleFactory for Unbuildable {
            fn type_id(&self) -> &'static str {
                "unbuildable"
            }
            fn build(
                &self,
                _: u32,
                _: &serde_json::Value,
            ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
                panic!("a terse index must never build a module")
            }
        }
        let mut registry = ModuleRegistry::new();
        registry.register(Unbuildable);
        let index =
            ModuleTypeIndex::from_registry(&registry, RegistryScope::Running, None).unwrap();
        assert_eq!(index.types, ["unbuildable"]);
        for n in 0..MAX_DISCOVERY_TYPES {
            registry.register_boxed(format!("unbuildable_{n}"), std::sync::Arc::new(Unbuildable));
        }
        let error = ModuleTypeList::from_registry(
            &registry,
            48_000,
            RegistryScope::Running,
            &ModuleTypeQuery {
                detail: TypeDetail::Full,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, RpcErrorCode::ResponseTooLarge);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sink_inspection_does_not_overwrite_files_or_launch_streaming() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.wav");
        std::fs::write(&path, b"preserve existing recording").unwrap();
        let registry = ModuleRegistry::default().for_inspection();
        let result = registry
            .build(
                "audio_file_sink",
                48_000,
                &serde_json::json!({"path": path}),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"preserve existing recording"
        );
        assert!(result.module.module().inputs().contains(&"audio_left"));
        assert!(result.control_surface.is_none());
        // A nonexistent executable proves that inspection never starts or even
        // probes ffmpeg. Both sinks validate config without activating outputs.
        for (name, config) in [
            (
                "rtmp_sink",
                serde_json::json!({"url": "rtmp://localhost/live/test", "resolution": "1280x720", "ffmpeg_path": "/nonexistent/ffmpeg"}),
            ),
            (
                "youtube_sink",
                serde_json::json!({"stream_key": "inspection-test", "ffmpeg_path": "/nonexistent/ffmpeg"}),
            ),
        ] {
            let result = registry.build(name, 48_000, &config).unwrap();
            assert!(result.module.module().outputs().contains(&"audio_right"));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn development_inspection_preserves_nested_sink_safety() {
        use crate::invention::development::DevelopmentFactory;
        use std::sync::{Arc, Mutex};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("must-not-be-created.wav");
        let definition = serde_json::from_value(serde_json::json!({
            "version": "1.0.0",
            "modules": [{"id": "recorder", "type": "audio_file_sink", "config": {"path": path}}],
            "connections": [],
            "inputs": [{"name": "audio", "to": "recorder", "to_port": "audio"}],
            "outputs": [{"name": "audio", "from": "recorder", "from_port": "audio"}]
        }))
        .unwrap();
        let mut registry = ModuleRegistry::default();
        let factory = DevelopmentFactory {
            name: "recorder_development".into(),
            definition,
            registry: registry.clone(),
            registered: Arc::new(Mutex::new(Default::default())),
        };
        registry.register_boxed("recorder_development", Arc::new(factory));
        let result = registry
            .for_inspection()
            .build("recorder_development", 48_000, &serde_json::Value::Null)
            .unwrap();
        assert_eq!(result.module.module().inputs(), ["audio"]);
        assert_eq!(result.module.module().outputs(), ["audio"]);
        assert!(!path.exists());
    }

    #[test]
    fn exact_filters_are_sorted_deduplicated_and_validated() {
        let registry = ModuleRegistry::default();
        let names = ["mixer".into(), "divisi".into(), "mixer".into()];
        let index =
            ModuleTypeIndex::from_registry(&registry, RegistryScope::Builtins, Some(&names))
                .unwrap();
        assert_eq!(index.types, ["divisi", "mixer"]);
        for (names, code) in [
            (vec![], RpcErrorCode::InvalidRequest),
            (
                vec!["mixer".into(); MAX_DISCOVERY_TYPES + 1],
                RpcErrorCode::InvalidRequest,
            ),
            (
                vec!["mixer".into(), "unknown".into()],
                RpcErrorCode::UnknownModuleType,
            ),
        ] {
            assert_eq!(
                ModuleTypeIndex::from_registry(&registry, RegistryScope::Builtins, Some(&names))
                    .unwrap_err()
                    .code,
                code
            );
        }
    }

    #[test]
    fn selector_and_config_limits_are_explicit() {
        let too_long = DescribeModuleQuery {
            module_type: Some("x".repeat(257)),
            ..Default::default()
        };
        assert_eq!(
            too_long.validate().unwrap_err().code,
            RpcErrorCode::InvalidRequest
        );
        let too_large = DescribeModuleQuery {
            module_type: Some("code".into()),
            config: Some(serde_json::json!({"script":"x".repeat(16_384)})),
            ..Default::default()
        };
        assert_eq!(
            too_large.validate().unwrap_err().code,
            RpcErrorCode::InvalidRequest
        );
    }

    #[test]
    fn discovery_response_versions_do_not_collide_with_rpc_envelopes() {
        let registry = ModuleRegistry::default();
        let discovery = ModuleTypeList::from_registry(
            &registry,
            48_000,
            RegistryScope::Builtins,
            &ModuleTypeQuery::default(),
        )
        .unwrap();
        let description = ModuleDescription::from_registry(
            &registry,
            48_000,
            RegistryScope::Builtins,
            &DescribeModuleQuery {
                module_type: Some("mixer".into()),
                ..Default::default()
            },
        )
        .unwrap();
        for payload in [
            super::super::RpcResponsePayload::ModuleTypes { discovery },
            super::super::RpcResponsePayload::ModuleDescription { description },
        ] {
            let response = super::super::RpcResponse::ok(Some("discovery".into()), payload);
            let encoded = serde_json::to_string(&response).unwrap();
            assert_eq!(
                serde_json::from_str::<super::super::RpcResponse>(&encoded).unwrap(),
                response
            );
        }
    }

    #[test]
    fn oversized_results_are_errors() {
        let error = check_discovery_size(&"x".repeat(MAX_DISCOVERY_RESPONSE_BYTES)).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::ResponseTooLarge);
    }
}
