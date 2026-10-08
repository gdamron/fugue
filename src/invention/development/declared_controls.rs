//! A development's exposed controls that reach declared inner controls.
//!
//! An exposed key aliases one or more inner controls (an explicit fan-out:
//! one `decay` reaching every voice). When any of them is declared, the key
//! is declared on the development too, with that first declared alias's
//! kind: a write to it is one request for the development, applied on the
//! audio thread by fanning out to every declared alias. Aliases still on
//! the legacy path are written by the development's surface directly, as
//! before. The inner declared surfaces are bound to [`Route::Inner`]: their
//! modules are reached only through the development.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::control_request::{
    apply_declared, ControlCells, ControlDecl, ControlIndex, ControlTable, Refusal, RtValue,
};
use crate::invention::declared::{DeclaredSurface, Route};
use crate::invention::runtime::ControlSurfaceInstance;
use crate::{GraphModule, Invention};

/// The declared aliases behind each of a development's declared controls.
pub(super) struct DevelopmentControls {
    table: ControlTable,
    cells: Arc<ControlCells>,
    /// Per declared control, in table order: each alias's inner module
    /// index and control.
    aliases: Vec<Vec<(usize, ControlIndex)>>,
}

impl DevelopmentControls {
    /// Declares every exposed key with a declared alias, or `None` when no
    /// alias is declared. Also returns the development surface's declared
    /// part, sharing the cells.
    pub(super) fn new(
        definition: &Invention,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
        module_indexes: &HashMap<String, usize>,
    ) -> Result<Option<(Self, DeclaredSurface)>, String> {
        let mut decls: Vec<ControlDecl> = Vec::new();
        let mut current = Vec::new();
        let mut aliases: Vec<Vec<(usize, ControlIndex)>> = Vec::new();
        for control in &definition.controls {
            let Some(found) = surfaces
                .get(&control.module)
                .and_then(|surface| surface.declaration(&control.control))
            else {
                continue;
            };
            let module = *module_indexes
                .get(&control.module)
                .ok_or_else(|| format!("Unknown control module: {}", control.module))?;
            let position = match decls.iter().position(|decl| decl.key == control.key) {
                Some(position) => position,
                None => {
                    decls.push(ControlDecl {
                        key: Cow::Owned(control.key.clone()),
                        indexed: false,
                        count: 1,
                        ..found.decl
                    });
                    current.push(found.current);
                    aliases.push(Vec::new());
                    decls.len() - 1
                }
            };
            aliases[position].push((module, found.index));
        }
        if decls.is_empty() {
            return Ok(None);
        }
        let table =
            ControlTable::built(decls).map_err(|why| format!("Development controls: {why}"))?;
        let cells = Arc::new(ControlCells::new(current));
        let surface = DeclaredSurface::new(table.clone(), cells.clone());
        let controls = Self {
            table,
            cells,
            aliases,
        };
        Ok(Some((controls, surface)))
    }

    pub(super) fn declared(&self) -> (&ControlTable, &ControlCells) {
        (&self.table, &self.cells)
    }

    /// Applies `value` to every declared alias of `control` in turn,
    /// publishing each inner control's new value. Returns what the first
    /// alias then holds, or its refusal. Audio thread: allocation-, free-
    /// and lock-free as long as the inner modules' `apply` are.
    pub(super) fn apply(
        &self,
        modules: &mut [GraphModule],
        control: ControlIndex,
        value: RtValue,
    ) -> Result<RtValue, Refusal> {
        let aliases = self
            .aliases
            .get(usize::from(control.0))
            .ok_or(Refusal::Unsupported)?;
        let mut first = None;
        for &(module, inner) in aliases {
            let module = modules.get_mut(module).ok_or(Refusal::TargetGone)?;
            let module = module.module_mut();
            let result = apply_declared(module, inner, value).and_then(|()| {
                let (_, cells) = module.declared().ok_or(Refusal::Unsupported)?;
                cells.load(inner).ok_or(Refusal::Unsupported)
            });
            first.get_or_insert(result);
        }
        first.unwrap_or(Err(Refusal::Unsupported))
    }
}

/// Binds every inner module's declared surface to [`Route::Inner`],
/// applying the controls written while it was built: from here on its
/// module is reached only through the development.
pub(super) fn enclose(
    surfaces: &IndexMap<String, ControlSurfaceInstance>,
    modules: &mut [(String, GraphModule)],
) {
    for (id, module) in modules {
        if let Some(surface) = surfaces.get(id) {
            surface.bind(Route::Inner, module.module_mut());
        }
    }
}
