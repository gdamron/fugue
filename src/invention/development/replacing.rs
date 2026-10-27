//! What a development's exposed controls replace (see
//! [`ConfigKey::replacing`]): an exposed key reaching a whole value replaces
//! the exposed keys reaching what that value replaces in the same inner
//! module, so a development exposing a melody's scale and its count records
//! and reloads them as the melody itself does.

use std::sync::{Mutex, OnceLock};

#[cfg(debug_assertions)]
use super::declared_controls::DevelopmentControls;
#[cfg(debug_assertions)]
use crate::control_request::{ControlCells, ControlIndex};
use crate::module_config::{names, replaced_by, ConfigKey, ConfigKind};
use crate::{Invention, ModuleRegistry};

/// One derived key: its exposed name, kind, and the exposed names it
/// replaces.
type Derived = (String, ConfigKind, Vec<String>);

/// The config keys of a development defined by `definition`, its inner
/// module types read from `registry`: only those replacing others.
pub(super) fn config_keys(
    definition: &Invention,
    registry: &ModuleRegistry,
) -> &'static [ConfigKey] {
    let type_of = |module: &str| {
        let spec = definition.modules.iter().find(|spec| spec.id == module);
        spec.map(|spec| spec.module_type.as_str())
    };
    let mut derived: Vec<Derived> = Vec::new();
    for control in &definition.controls {
        let Some(inner) = type_of(&control.module).map(|t| registry.config_keys(t)) else {
            continue;
        };
        let Some(declared) = inner.iter().find(|key| key.key == control.control) else {
            continue;
        };
        let replaces = replaced_by(inner, &control.control);
        // A key fanning out to anything the value does not replace (in
        // another module, say) is only partly replaced: its config holds
        // one value, so it keeps it.
        let within = |name: &str| {
            let mut targets = (definition.controls.iter()).filter(|other| other.name == name);
            targets.all(|other| {
                other.module == control.module
                    && replaces.iter().any(|key| names(key, &other.control))
            })
        };
        let reached = (definition.controls.iter())
            .filter(|other| other.name != control.name && within(&other.name))
            .map(|other| other.name.clone());
        let at = match derived.iter().position(|(name, ..)| *name == control.name) {
            Some(at) => at,
            None => {
                derived.push((control.name.clone(), declared.kind, Vec::new()));
                derived.len() - 1
            }
        };
        for name in reached {
            if !derived[at].2.contains(&name) {
                derived[at].2.push(name);
            }
        }
    }
    derived.retain(|(_, _, replaces)| !replaces.is_empty());
    intern(derived)
}

/// `derived` as config keys living as long as a type's own: leaked once per
/// distinct set, so building a development again leaks nothing more.
fn intern(derived: Vec<Derived>) -> &'static [ConfigKey] {
    if derived.is_empty() {
        return &[];
    }
    static INTERNED: OnceLock<Mutex<Vec<&'static [ConfigKey]>>> = OnceLock::new();
    let mut interned = INTERNED.get_or_init(Default::default).lock().unwrap();
    let same = |keys: &&&'static [ConfigKey]| {
        keys.len() == derived.len()
            && keys
                .iter()
                .zip(&derived)
                .all(|(key, (name, kind, replaces))| {
                    key.key == name
                        && key.kind == *kind
                        && key
                            .replaces
                            .iter()
                            .copied()
                            .eq(replaces.iter().map(String::as_str))
                })
    };
    if let Some(keys) = interned.iter().find(same) {
        return keys;
    }
    let leak = |name: String| -> &'static str { Box::leak(name.into_boxed_str()) };
    let keys = derived.into_iter().map(|(name, kind, replaces)| ConfigKey {
        key: leak(name),
        kind,
        replaces: Box::leak(replaces.into_iter().map(leak).collect()),
    });
    let keys: &'static [ConfigKey] = Box::leak(keys.collect());
    interned.push(keys);
    keys
}

/// Panics unless what each exposed key's writes rewrite in the inner
/// modules it reaches agrees with what `keys` declares it replaces, as the
/// registry checks a module's own table: a rewritten exposed key (every
/// inner control it reaches rewritten) is declared or not held by `config`,
/// and every declared one is rewritten. Inner modules are told apart by
/// their cells, since a type's modules share one table. A key reaching a
/// nested development is left unchecked: its rewrites lie deeper than the
/// keys exposed here reach. For debug builds.
#[cfg(debug_assertions)]
pub(super) fn check_rewrites(
    definition: &Invention,
    name: &str,
    keys: &[ConfigKey],
    config: &serde_json::Value,
    controls: &DevelopmentControls,
    modules: &[crate::GraphModule],
) {
    let (table, _) = controls.declared();
    let module = |index: usize| modules[index].module();
    let cells = |index: usize| {
        module(index)
            .declared()
            .map(|(_, cells)| cells as *const ControlCells)
    };
    let keyed = |index: usize| {
        table
            .decl(ControlIndex(index as u16))
            .map(|(decl, _)| decl.key.as_ref())
    };
    let targets: Vec<Vec<_>> = (0..table.len())
        .map(|index| controls.targets(ControlIndex(index as u16)))
        .map(|targets| targets.into_iter().map(|(m, i)| (cells(m), i)).collect())
        .collect();
    // A key reaching an undeclared control is never wholly
    // rewritten: the walk cannot see it.
    let listed = |key: &str| {
        (definition.controls.iter())
            .filter(|c| c.name == key)
            .count()
    };
    let whole = |index: usize| {
        let count = targets[index].len();
        count > 0 && keyed(index).is_some_and(|key| listed(key) == count)
    };
    for index in 0..table.len() {
        let control = ControlIndex(index as u16);
        let (Some(key), direct) = (keyed(index), controls.targets(control)) else {
            continue;
        };
        if direct.iter().any(|&(m, _)| module(m).enclosure().is_some()) {
            continue;
        }
        let mut reached = Vec::new();
        for (m, inner) in direct {
            module(m).for_each_alias(inner, &mut |_, cells, alias| {
                reached.push((Some(cells as *const ControlCells), alias));
            });
        }
        let rewritten: Vec<&str> = (0..table.len())
            .filter(|&other| other != index && whole(other))
            .filter(|&other| targets[other].iter().all(|target| reached.contains(target)))
            .filter_map(keyed)
            .collect();
        let replaced = replaced_by(keys, key);
        for other in &rewritten {
            assert!(
                replaced.contains(other) || config.get(*other).is_none(),
                "{name}: a write to '{key}' rewrites '{other}', which its config key does not \
                 declare it replaces"
            );
        }
        let exposed = |other: &&&str| (0..table.len()).any(|index| keyed(index) == Some(**other));
        for other in replaced.iter().filter(exposed) {
            assert!(
                rewritten.contains(other),
                "{name}: config key '{key}' declares it replaces '{other}', which a write to it \
                 does not rewrite"
            );
        }
    }
}
