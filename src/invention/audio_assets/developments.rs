//! Which of a module's asset slots name audio when the module is a
//! development instance: its config holds the controls its development
//! exposes, so a slot's key names audio only when the control exposed
//! under it does in turn.

use super::module_asset_slots;
use crate::invention::format::ModuleSpec;
use crate::invention::reload::LoadedDevelopments;

/// The audio asset slots of `module`'s config. A development instance's
/// config holds the controls its development exposes, so a slot's key
/// names audio only when the control exposed under it does in turn: an
/// event the development calls `asset` is not read as an asset.
pub(crate) fn audio_slots<'a>(
    module: &'a mut ModuleSpec,
    developments: &LoadedDevelopments,
) -> impl Iterator<Item = &'a mut serde_json::Value> {
    let module_type = &module.module_type;
    let names_audio: Vec<&'static str> = ["asset", "samples", "zones"]
        .into_iter()
        .filter(|key| names_audio(developments, module_type, key, 0))
        .collect();
    module_asset_slots(&mut module.config)
        .filter(move |(key, _)| names_audio.contains(key))
        .map(|(_, value)| value)
}

/// How deep [`names_audio`] follows developments within developments.
const MAX_NESTING: usize = 32;

/// Whether config key `key` of a module of `module_type`, built where
/// `developments` are declared, names audio: it does for any module but a
/// development, and for a development when every control it exposes as
/// `key` is an inner module's `key` naming audio in turn.
fn names_audio(
    developments: &LoadedDevelopments,
    module_type: &str,
    key: &str,
    depth: usize,
) -> bool {
    let Some(development) = developments.get(module_type) else {
        return true;
    };
    let definition = &development.definition;
    let mut exposed = (definition.controls.iter())
        .filter(|control| control.name == key)
        .peekable();
    exposed.peek().is_some()
        && depth < MAX_NESTING
        && exposed.all(|control| {
            let inner = definition.modules.iter().find(|m| m.id == control.module);
            inner.is_some_and(|inner| {
                // An inner development is declared in this development's
                // scope or inherited from where it is used.
                let scope = match development.scope.get(&inner.module_type) {
                    Some(_) => &*development.scope,
                    None => developments,
                };
                control.control == key && names_audio(scope, &inner.module_type, key, depth + 1)
            })
        })
}
