//! Package inventory responses.

use crate::ModuleRegistry;
use serde::{Deserialize, Serialize};

/// Built-in and future package inventory response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageList {
    pub packages: Vec<PackageInfo>,
}

impl PackageList {
    /// Returns the package inventory available from the built-in module registry.
    pub fn built_in(registry: &ModuleRegistry) -> Self {
        let mut module_types: Vec<String> = registry.types().map(str::to_string).collect();
        module_types.sort();
        Self {
            packages: vec![PackageInfo {
                name: "builtin".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                source: PackageSource::BuiltIn,
                module_types,
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub source: PackageSource,
    pub module_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PackageSource {
    BuiltIn,
    External,
}
