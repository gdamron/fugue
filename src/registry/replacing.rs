//! Config keys whose values rewrite others wholesale (see
//! [`ConfigKey::replacing`]): kept per type for the paths that record a
//! config without a registry at hand, and checked against what a module's
//! own writes rewrite at runtime.

use std::collections::HashMap;

use super::ModuleRegistry;
use crate::control_request::ControlIndex;
use crate::factory::ModuleBuildResult;
use crate::module_config::{replaced_by, ConfigKey};

/// The config keys of each type that declares a replacing key, as a
/// registry declares them.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReplacingKeys(HashMap<String, &'static [ConfigKey]>);

impl ReplacingKeys {
    pub(crate) fn of(registry: &ModuleRegistry) -> Self {
        let replacing = (registry.types())
            .map(|type_id| (type_id, registry.config_keys(type_id)))
            .filter(|(_, keys)| keys.iter().any(|key| !key.replaces.is_empty()))
            .map(|(type_id, keys)| (type_id.to_string(), keys));
        Self(replacing.collect())
    }

    /// The config keys `type_id` declares, when any replaces others; none
    /// otherwise, as none of them replaces anything.
    pub(crate) fn config_keys(&self, type_id: &str) -> &'static [ConfigKey] {
        self.0.get(type_id).copied().unwrap_or(&[])
    }
}

/// Panics unless what each of a built module's controls rewrites within its
/// own table at runtime (`Module::for_each_alias`) agrees with what its
/// config key declares it replaces, so the recorded config and the module
/// playing it never disagree: every replaced key is rewritten, and a key
/// rewritten beyond them is one `config` does not hold, a value derived
/// from what it does hold (a melody's whole scale rederives the weights
/// not written to single positions, and keeps the written ones). For debug
/// builds: a mismatch is a programming error.
pub(super) fn check_rewrites(
    type_id: &str,
    keys: &[ConfigKey],
    config: &serde_json::Value,
    built: &ModuleBuildResult,
) {
    let module = built.module.module();
    let Some((table, _)) = module.declared() else {
        return;
    };
    for index in 0..table.len() {
        let control = ControlIndex(index as u16);
        let Some((decl, _)) = table.decl(control) else {
            continue;
        };
        let replaced = replaced_by(keys, &decl.key);
        let mut rewritten: Vec<&str> = Vec::new();
        module.for_each_alias(control, &mut |aliased, _, alias| {
            // A development's aliases live in its inner modules' tables.
            if !std::ptr::eq(aliased, table) {
                return;
            }
            let Some((alias, position)) = table.decl(alias) else {
                return;
            };
            let key = alias.key.as_ref();
            let named = match alias.indexed {
                true => format!("{key}.{position}"),
                false => key.to_string(),
            };
            assert!(
                replaced.contains(&key) || config.get(&named).is_none(),
                "{type_id}: a write to '{}' rewrites '{named}', which its config key does not \
                 declare it replaces",
                decl.key
            );
            if !rewritten.contains(&key) {
                rewritten.push(key);
            }
        });
        for key in replaced {
            assert!(
                rewritten.contains(key),
                "{type_id}: config key '{}' declares it replaces '{key}', which a write to it \
                 does not rewrite",
                decl.key
            );
        }
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use crate::module_config::ConfigKey;
    use crate::test_support::tape::TapeFactory;
    use crate::{ModuleBuildResult, ModuleFactory, ModuleRegistry};

    /// A tape whose notes' config key does not declare the gain they
    /// rewrite.
    struct Undeclared;

    impl ModuleFactory for Undeclared {
        fn type_id(&self) -> &'static str {
            "undeclared"
        }

        fn config_keys(&self) -> &'static [ConfigKey] {
            const { &[ConfigKey::float("gain"), ConfigKey::json("notes")] }
        }

        fn build(
            &self,
            sample_rate: u32,
            config: &serde_json::Value,
        ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
            TapeFactory.build(sample_rate, config)
        }
    }

    #[test]
    #[should_panic(expected = "a write to 'notes' rewrites 'gain'")]
    fn a_rewrite_its_config_key_does_not_declare_is_caught() {
        let mut registry = ModuleRegistry::default();
        registry.register(Undeclared);
        let _ = registry.build("undeclared", 48_000, &serde_json::json!({ "gain": 0.5 }));
    }

    #[test]
    fn a_declared_rewrite_builds() {
        let mut registry = ModuleRegistry::default();
        registry.register(TapeFactory);
        assert!(registry
            .build("tape", 48_000, &serde_json::json!({}))
            .is_ok());
    }
}
