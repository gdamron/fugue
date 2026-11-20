//! What a document holds that a preview reports on without playing it: the
//! modules that pick by chance and the scripts and agents it does not run,
//! inside developments too.

use crate::{DevelopmentSpec, Invention};

/// Module types that draw on a random stream.
const CHANCE_TYPES: [&str; 3] = ["chance", "melody", "noise"];

/// Module types whose script or agent a preview never starts.
const HOSTED_TYPES: [&str; 2] = ["code", "agent"];

/// A module found in the document, by the id it has as built: an inner
/// module of a development instance is `instance/inner`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Found {
    pub(super) module_id: String,
    pub(super) module_type: String,
}

/// What [`walk`] found.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Walk {
    pub(super) chance: Vec<String>,
    pub(super) not_run: Vec<Found>,
}

/// Walks `invention`'s modules and those of the inline developments they
/// instantiate, in document order. A development given by `path` or `ref`
/// is not read here: a document passed as JSON has no folder or catalog to
/// resolve those from (the browser engine refuses them), so only an
/// absolute `path` could build, and its modules go unlisted.
pub(super) fn walk(invention: &Invention) -> Walk {
    let mut found = Walk::default();
    visit(invention, "", &[], &mut found, 0);
    found
}

/// Developments nest at most this deep, so a definition that names itself
/// cannot loop the walk.
const MAX_DEPTH: usize = 16;

fn visit(
    invention: &Invention,
    prefix: &str,
    outer: &[&[DevelopmentSpec]],
    found: &mut Walk,
    depth: usize,
) {
    let mut scopes: Vec<&[DevelopmentSpec]> = vec![&invention.developments];
    scopes.extend_from_slice(outer);
    for module in &invention.modules {
        let id = format!("{prefix}{}", module.id);
        let kind = module.module_type.as_str();
        if CHANCE_TYPES.contains(&kind) {
            found.chance.push(id.clone());
        }
        if HOSTED_TYPES.contains(&kind) {
            found.not_run.push(Found {
                module_id: id.clone(),
                module_type: kind.to_string(),
            });
        }
        let definition = scopes
            .iter()
            .flat_map(|scope| scope.iter())
            .find(|spec| spec.name == kind)
            .and_then(|spec| spec.definition.as_deref());
        if let (Some(definition), true) = (definition, depth < MAX_DEPTH) {
            visit(definition, &format!("{id}/"), &scopes, found, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_chance_and_hosted_modules_inside_developments() {
        let invention: Invention = serde_json::from_value(serde_json::json!({
            "modules": [
                { "id": "tune", "type": "melody" },
                { "id": "s", "type": "code" },
                { "id": "inst", "type": "pair" }
            ],
            "connections": [],
            "developments": [{ "name": "pair", "definition": {
                "modules": [
                    { "id": "dice", "type": "chance" },
                    { "id": "bot", "type": "agent" }
                ],
                "connections": []
            }}]
        }))
        .unwrap();
        let found = walk(&invention);
        assert_eq!(found.chance, ["tune", "inst/dice"]);
        let not_run: Vec<_> = found
            .not_run
            .iter()
            .map(|f| (f.module_id.as_str(), f.module_type.as_str()))
            .collect();
        assert_eq!(not_run, [("s", "code"), ("inst/bot", "agent")]);
    }
}
