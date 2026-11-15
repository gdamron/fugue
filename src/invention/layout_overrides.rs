//! Overrides: where a piece moved modules inside a development it borrows.
//!
//! A development from an installed package is never written (its version
//! is immutable). A piece that uses one can still rearrange its inside: the
//! move is saved in the piece's own layout as an override, keyed by the
//! path into the development and the module id (`"voice/filter"`, or
//! `"voice/env/attack"` inside a development the voice uses), and layered on
//! the author's arrangement wherever it is shown. Removing the overrides is
//! going back to the original.
//!
//! Like the rest of a layout, an override is never heard. When the original
//! changes (the piece moves to a new package version) and an override names
//! a development or module that is no longer there, load and reload drop it
//! and say which, as pins are dropped when the graph changes.

use std::collections::BTreeMap;

use super::layout::{Layout, Position};
use super::reload::LoadedDevelopments;
use crate::Invention;

/// The separator between the names of an override's key.
const SEPARATOR: char = '/';

/// The override key of module `id` inside the development `path` names, or
/// `None` when a name holds the separator, which no key can spell.
pub(crate) fn key(path: &[String], id: &str) -> Option<String> {
    let names = path.iter().map(String::as_str).chain([id]);
    let mut key = String::new();
    for (index, name) in names.enumerate() {
        if name.is_empty() || name.contains(SEPARATOR) {
            return None;
        }
        if index > 0 {
            key.push(SEPARATOR);
        }
        key.push_str(name);
    }
    Some(key)
}

/// The development path and module id `key` names, when it names both.
fn split(key: &str) -> Option<(Vec<String>, &str)> {
    let (path, id) = key.rsplit_once(SEPARATOR)?;
    let path: Vec<String> = path.split(SEPARATOR).map(str::to_string).collect();
    if id.is_empty() || path.iter().any(String::is_empty) {
        return None;
    }
    Some((path, id))
}

impl Layout {
    /// The overrides inside the development `path` names, by module id.
    pub(crate) fn overrides_in(&self, path: &[String]) -> BTreeMap<String, Position> {
        (self.overrides.iter())
            .filter_map(|(key, position)| {
                let (within, id) = split(key)?;
                (within == path).then(|| (id.to_string(), *position))
            })
            .collect()
    }
}

/// The module ids inside the development `path` names among `keys`, the
/// overrides a load dropped.
pub(crate) fn dropped_in(keys: &[String], path: &[String]) -> Vec<String> {
    (keys.iter())
        .filter_map(|key| split(key).filter(|(within, _)| within == path))
        .map(|(_, id)| id.to_string())
        .collect()
}

/// `layout` (the author's) with `overrides` on top: an override moves a
/// module the definition has; one naming another is not shown.
pub(crate) fn layered(
    definition: &Invention,
    overrides: &BTreeMap<String, Position>,
) -> Option<Invention> {
    if overrides.is_empty() {
        return None;
    }
    let mut shown = definition.clone();
    for (id, position) in overrides {
        if definition.modules.iter().any(|spec| &spec.id == id) {
            shown.layout.modules.insert(id.clone(), *position);
        }
    }
    Some(shown)
}

/// An override the piece's settling dropped: its key and the warning.
pub(crate) struct Dropped {
    pub(crate) key: String,
    pub(crate) message: String,
}

impl Invention {
    /// Drops each override of this document (the piece) that no longer
    /// places a module of a development it borrows, as `loaded` resolves
    /// them: its development is gone, is no longer borrowed, or no longer
    /// has the module, or its position is not finite. The piece loads all
    /// the same; each drop is returned for the load to report.
    pub(crate) fn settle_overrides(&mut self, loaded: &LoadedDevelopments) -> Vec<Dropped> {
        let mut dropped = Vec::new();
        self.layout.overrides.retain(|key, position| {
            let why = match split(key) {
                None => Some("its key names no module inside a development".to_string()),
                Some((path, id)) => match loaded.find(&path) {
                    Err(_) => Some(format!(
                        "the piece no longer uses a development '{}'",
                        path.join("/")
                    )),
                    Ok(entry) if !super::edits::borrowed(&entry.home) => Some(format!(
                        "development '{}' is no longer borrowed; it is arranged in its own \
                         document",
                        path.join("/")
                    )),
                    Ok(entry) if !entry.definition.modules.iter().any(|m| m.id == id) => {
                        Some(format!(
                            "development '{}' no longer has a module '{id}'",
                            path.join("/")
                        ))
                    }
                    Ok(_) if !position.is_finite() => Some("its position is not finite".into()),
                    Ok(_) => None,
                },
            };
            let Some(why) = why else {
                return true;
            };
            dropped.push(Dropped {
                key: key.clone(),
                message: format!("layout override '{key}' was dropped: {why}"),
            });
            false
        });
        dropped
    }
}

#[cfg(test)]
#[path = "layout_overrides_tests.rs"]
mod tests;
