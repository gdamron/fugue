//! The development definitions a document declares, as loaded, and which of
//! them changed between two loads of a document.

use indexmap::IndexMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::rpc::{DevelopmentDeclaration, RpcError, RpcErrorCode};
use crate::Invention;

use crate::invention::builder::load_development_definition;

/// Development definitions as loaded for a document, by declaration scope
/// (see [`LoadedDevelopments`]). Path-based definitions are captured at load
/// time so a later reload can detect that a file changed on disk, nested ones
/// included.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentDefinitions {
    pub(super) loaded: Arc<LoadedDevelopments>,
}

impl DevelopmentDefinitions {
    /// Recursively loads every development definition reachable from a
    /// document, including path-based definitions nested inside other
    /// definitions, each within the scope that declares it.
    pub fn resolve(document: &Invention) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self::of(Arc::new(LoadedDevelopments::load(document)?)))
    }

    /// The definitions `loaded`, kept for a later reload to compare against.
    pub(crate) fn of(loaded: Arc<LoadedDevelopments>) -> Self {
        Self { loaded }
    }

    /// The developments the running document declares.
    pub(crate) fn loaded(&self) -> &Arc<LoadedDevelopments> {
        &self.loaded
    }

    /// How the development `path` names is declared, and its definition as
    /// loaded (see [`LoadedDevelopments::find`]).
    pub fn development(
        &self,
        path: &[String],
    ) -> Result<(DevelopmentDeclaration, &Invention), RpcError> {
        let found = self.loaded.find(path)?;
        Ok((found.declared, &found.definition))
    }
}

/// The developments one document declares, each loaded with the ones its
/// own definition declares, in declaration order. A development built after
/// load (a nested one, or one an edit adds) builds from the declaration in
/// scope where it is used, as loaded, never from disk again. Two siblings
/// may each declare a different nested development under the same alias.
///
/// A declaration whose alias an enclosing scope already registered is not
/// loaded: the builder keeps the inherited factory and never reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LoadedDevelopments(pub(super) IndexMap<String, LoadedDevelopment>);

/// One loaded development declaration.
#[derive(Debug, Clone)]
pub(crate) struct LoadedDevelopment {
    /// How the declaring document names its definition.
    pub(crate) declared: DevelopmentDeclaration,
    pub(crate) definition: Invention,
    /// The developments `definition` declares.
    pub(crate) scope: Arc<LoadedDevelopments>,
}

/// Two declarations build the same development when they differ at most in
/// how they are declared, the minimum core they declare, their layout or
/// their module names: saving
/// writes that minimum into the file, presentation builds nothing, and
/// reloading must not rebuild
/// instances nobody touched.
impl PartialEq for LoadedDevelopment {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.definition.as_built() == other.definition.as_built()
    }
}

impl LoadedDevelopments {
    /// Loads every development `document` declares, recursively.
    pub(crate) fn load(document: &Invention) -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_within(document, &HashSet::new(), &mut Vec::new())
    }

    /// Loads `document`'s declarations, skipping those whose alias is in
    /// `inherited`: the registry a nested build starts from already has
    /// them, and the builder keeps those (see `register_developments`).
    fn load_within(
        document: &Invention,
        inherited: &HashSet<String>,
        ancestry: &mut Vec<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut scope = IndexMap::new();
        for spec in &document.developments {
            if inherited.contains(&spec.name) || scope.contains_key(&spec.name) {
                continue;
            }
            let definition = load_development_definition(document, spec)?;
            // A development declared again inside its own definition is a
            // cycle: its scope is left empty there.
            let nested = if ancestry.contains(&spec.name) {
                Self::default()
            } else {
                // A development's factory starts from the registry as it was
                // when the development was registered: what this document
                // inherits, and the developments it declares before this one.
                let mut visible = inherited.clone();
                visible.extend(scope.keys().cloned());
                ancestry.push(spec.name.clone());
                let nested = Self::load_within(&definition, &visible, ancestry);
                ancestry.pop();
                nested?
            };
            let loaded = LoadedDevelopment {
                declared: DevelopmentDeclaration::of(spec),
                definition,
                scope: Arc::new(nested),
            };
            scope.insert(spec.name.clone(), loaded);
        }
        Ok(Self(scope))
    }

    /// The declaration of `name` in this scope.
    pub(crate) fn get(&self, name: &str) -> Option<&LoadedDevelopment> {
        self.0.get(name)
    }

    /// The development `path` names, as loaded: its first name declared in
    /// this scope, and each next one visible inside the one before it, as
    /// its builder sees them: declared in that development's own
    /// definition, or inherited from an enclosing scope that declares it
    /// before the development that encloses it.
    pub(crate) fn find(&self, path: &[String]) -> Result<&LoadedDevelopment, RpcError> {
        let refuse = |message: String| RpcError::new(RpcErrorCode::InvalidRequest, message);
        let (mut scope, mut found) = (self, None);
        // What the current scope inherits, outermost first, in declaration
        // order.
        let mut inherited: Vec<(&str, &LoadedDevelopment)> = Vec::new();
        for (depth, name) in path.iter().enumerate() {
            let entry = match scope.0.get_full(name.as_str()) {
                Some((index, _, entry)) => {
                    let before = scope.0.iter().take(index);
                    inherited.extend(before.map(|(name, entry)| (name.as_str(), entry)));
                    entry
                }
                None => match inherited.iter().rposition(|(seen, _)| *seen == name) {
                    Some(index) => {
                        let entry = inherited[index].1;
                        inherited.truncate(index);
                        entry
                    }
                    None => {
                        return Err(refuse(match depth {
                            0 => format!("the piece declares no development named '{name}'"),
                            _ => format!(
                                "no development named '{name}' is visible inside '{}'",
                                path[..depth].join("/")
                            ),
                        }))
                    }
                },
            };
            scope = &entry.scope;
            found = Some(entry);
        }
        found.ok_or_else(|| refuse("name the development to read".into()))
    }
}

impl DevelopmentDeclaration {
    fn of(spec: &crate::invention::format::DevelopmentSpec) -> Self {
        match (&spec.reference, &spec.path) {
            (Some(_), _) => Self::Ref,
            (None, Some(_)) => Self::Path,
            (None, None) => Self::Inline,
        }
    }
}

/// Returns the top-level development type names whose definitions changed
/// between the previously loaded document and the new one: a development
/// changes when its definition or any development it declares (in its own
/// scope, recursively) changed, or, transitively, when it instantiates a
/// changed top-level development it inherits. A name with no known previous
/// definition counts as changed, so an unknown history degrades to
/// conservatively rebuilding every development instance.
pub(crate) fn changed_development_types(
    previous: &DevelopmentDefinitions,
    new: &DevelopmentDefinitions,
) -> HashSet<String> {
    let mut changed: HashSet<String> = new
        .loaded
        .0
        .iter()
        .filter(|(name, loaded)| previous.loaded.get(name) != Some(*loaded))
        .map(|(name, _)| name.clone())
        .collect();

    loop {
        let mut grew = false;
        for (name, loaded) in &new.loaded.0 {
            if !changed.contains(name) && uses_any(loaded, &changed) {
                changed.insert(name.clone());
                grew = true;
            }
        }
        if !grew {
            return changed;
        }
    }
}

/// Whether `loaded`, or any development it declares, instantiates one of
/// `names` as inherited from an enclosing scope rather than declared locally.
fn uses_any(loaded: &LoadedDevelopment, names: &HashSet<String>) -> bool {
    let visible: HashSet<String> = names
        .iter()
        .filter(|name| loaded.scope.get(name).is_none())
        .cloned()
        .collect();
    loaded
        .definition
        .modules
        .iter()
        .any(|module| visible.contains(&module.module_type))
        || loaded
            .scope
            .0
            .values()
            .any(|nested| uses_any(nested, &visible))
}
