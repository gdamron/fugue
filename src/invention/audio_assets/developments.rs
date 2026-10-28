//! Which of a module's asset slots and controls name audio when the module
//! is a development instance: its config holds the controls its development
//! exposes, so a key names audio only when the control exposed under it
//! does in turn, as its development declares it.

use super::{is_asset_control, module_asset_slots};
use crate::invention::format::{Invention, ModuleSpec};
use crate::invention::reload::LoadedDevelopments;
use crate::registry::ModuleRegistry;

/// Where a development type's declaration is found: the scopes a document
/// declares, from the innermost out, then the registry it is built from,
/// which holds the developments a nested build inherits from where its
/// development is used.
#[derive(Clone, Copy)]
pub(crate) struct Declarations<'a> {
    scope: &'a LoadedDevelopments,
    outer: Option<&'a Declarations<'a>>,
    registry: Option<&'a ModuleRegistry>,
}

impl<'a> Declarations<'a> {
    /// The developments `scope` declares, and those `registry` holds.
    pub(crate) fn of(scope: &'a LoadedDevelopments, registry: Option<&'a ModuleRegistry>) -> Self {
        Self {
            scope,
            outer: None,
            registry,
        }
    }

    /// Runs `inside` on the definition of development `module_type`, with
    /// the declarations its own modules see, or returns `None` when no
    /// development has that name.
    fn within<R>(
        &self,
        module_type: &str,
        inside: impl FnOnce(&Invention, &Declarations<'_>) -> R,
    ) -> Option<R> {
        let mut level = Some(self);
        while let Some(here) = level {
            if let Some(development) = here.scope.get(module_type) {
                let nested = Declarations {
                    scope: &development.scope,
                    outer: Some(here),
                    registry: here.registry,
                };
                return Some(inside(&development.definition, &nested));
            }
            level = here.outer;
        }
        let factory = self.registry?.development(module_type)?;
        let nested = Declarations::of(&factory.loaded, Some(&factory.registry));
        Some(inside(&factory.definition, &nested))
    }
}

/// The audio asset slots of `module`'s config. A development instance's
/// config holds the controls its development exposes, so a slot's key
/// names audio only when the control exposed under it does in turn: an
/// event the development calls `asset` is not read as an asset.
pub(crate) fn audio_slots<'a>(
    module: &'a mut ModuleSpec,
    declarations: &Declarations<'_>,
) -> impl Iterator<Item = &'a mut serde_json::Value> {
    let module_type = &module.module_type;
    let names_audio: Vec<&'static str> = ["asset", "samples", "zones"]
        .into_iter()
        .filter(|key| names_audio(declarations, module_type, key, 0))
        .collect();
    module_asset_slots(&mut module.config)
        .filter(move |(key, _)| names_audio.contains(key))
        .map(|(_, value)| value)
}

/// Whether control `key` of a module of `module_type` loads the audio it
/// names: an asset control (`asset`, `asset.N`) of a module, or of a
/// development whose every control exposed as `key` aliases one in turn,
/// whatever key the alias targets (an `asset` exposed from `asset.0`).
pub(crate) fn names_audio_control(
    declarations: &Declarations<'_>,
    module_type: &str,
    key: &str,
) -> bool {
    is_asset_control(key) && names_audio(declarations, module_type, key, 0)
}

/// How deep [`names_audio`] follows developments within developments.
const MAX_NESTING: usize = 32;

/// Whether config key `key` of a module of `module_type` names audio: it
/// does for a module's asset key (`asset`, `asset.N`, `samples`, `zones`),
/// and for a development's key when every control it exposes as `key`
/// targets an inner module's key naming audio in turn.
fn names_audio(
    declarations: &Declarations<'_>,
    module_type: &str,
    key: &str,
    depth: usize,
) -> bool {
    let exposes_audio = |definition: &Invention, inner: &Declarations<'_>| {
        let mut exposed = (definition.controls.iter())
            .filter(|control| control.name == key)
            .peekable();
        exposed.peek().is_some()
            && depth < MAX_NESTING
            && exposed.all(|control| {
                let module = definition.modules.iter().find(|m| m.id == control.module);
                module.is_some_and(|module| {
                    names_audio(inner, &module.module_type, &control.control, depth + 1)
                })
            })
    };
    declarations
        .within(module_type, exposes_audio)
        .unwrap_or_else(|| is_asset_control(key) || matches!(key, "samples" | "zones"))
}

/// Which controls of a running graph's modules load the audio they name,
/// as the registry the graph was built or last reloaded from declares its
/// developments (see [`names_audio_control`]).
#[derive(Clone, Default)]
pub(crate) struct AudioControls(Option<std::sync::Arc<ModuleRegistry>>);

impl AudioControls {
    pub(crate) fn of(registry: std::sync::Arc<ModuleRegistry>) -> Self {
        Self(Some(registry))
    }

    /// Whether control `key` of a module of `module_type` loads the audio
    /// it names.
    pub(crate) fn names_audio(&self, module_type: &str, key: &str) -> bool {
        let scope = LoadedDevelopments::default();
        let declarations = Declarations::of(&scope, self.0.as_deref());
        names_audio_control(&declarations, module_type, key)
    }
}

impl std::fmt::Debug for AudioControls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AudioControls")
    }
}
