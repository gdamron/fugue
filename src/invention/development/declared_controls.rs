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
//!
//! A declaration states exactly what its module accepts, and a development
//! is no exception: every later declared alias of a key must take every
//! value its first one does ([`takes_all`]), or the development fails to
//! build. So the alias taking the fewest values is listed first (a choice
//! with the fewest options, the narrowest integer range).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::control_request::{
    apply_declared, integer_domain, take_automation, ControlCells, ControlDecl, ControlIndex,
    ControlTable, DeclKind, Refusal, RtValue,
};
use crate::invention::declared::{DeclaredSurface, Route};
use crate::invention::runtime::ControlSurfaceInstance;
use crate::{GraphModule, Invention};

/// The declared aliases behind each of a development's declared controls.
pub(super) struct DevelopmentControls {
    table: ControlTable,
    cells: Arc<ControlCells>,
    /// Per declared control, in table order: each declared alias.
    aliases: Vec<Vec<Alias>>,
}

/// One inner declared control an exposed key reaches.
struct Alias {
    module: usize,
    index: ControlIndex,
    /// Its own kind, so a value converts to what it holds; `None` when the
    /// key's value can never be one (a payload).
    kind: Option<DeclKind>,
    /// Its own clamp: an integer that clamps takes any whole number in its
    /// domain ([`integer_domain`]), clamping it as it applies.
    clamp: Option<(f32, f32)>,
    /// For a choice reached by a choice: each of the key's options as this
    /// alias numbers it, by name (`None` where it has no such option).
    choices: Box<[Option<u32>]>,
}

/// `value` as a control of `kind` (clamping to `clamp`) takes it: a number
/// and an integer convert (a fraction never becomes an integer, and a whole
/// number must lie in the integer's domain); anything else must match.
fn convert(
    value: RtValue,
    kind: DeclKind,
    clamp: Option<(f32, f32)>,
    choices: &[Option<u32>],
) -> Option<RtValue> {
    let domain = |min, max| integer_domain(min, max, clamp);
    match (kind, value) {
        (DeclKind::Choice(_), RtValue::U32(position)) => choices
            .get(position as usize)
            .copied()
            .flatten()
            .map(RtValue::U32),
        (DeclKind::Number { .. }, RtValue::F32(_)) => Some(value),
        (DeclKind::Number { .. }, RtValue::I32(whole)) => Some(RtValue::F32(whole as f32)),
        (DeclKind::Integer { min, max }, RtValue::I32(whole)) => {
            let (low, high) = domain(min, max);
            (low..=high).contains(&whole).then_some(value)
        }
        (DeclKind::Integer { min, max }, RtValue::F32(number)) => {
            let (low, high) = domain(min, max);
            let whole = number.fract() == 0.0 && number >= low as f32 && number <= high as f32;
            whole.then_some(RtValue::I32(number as i32))
        }
        (DeclKind::Bool, RtValue::Bool(_)) => Some(value),
        _ => None,
    }
}

/// Whether control `alias` takes every value control `first` does, as
/// [`convert`] hands them over: any number (each clamps for itself) or
/// whole number, an integer domain holding `first`'s, every option by name.
fn takes_all(first: &ControlDecl, alias: &ControlDecl) -> bool {
    match (first.kind, alias.kind) {
        (DeclKind::Number { .. } | DeclKind::Integer { .. }, DeclKind::Number { .. }) => true,
        (
            DeclKind::Integer { min, max },
            DeclKind::Integer {
                min: own_min,
                max: own_max,
            },
        ) => {
            let (min, max) = integer_domain(min, max, first.clamp);
            let (low, high) = integer_domain(own_min, own_max, alias.clamp);
            low <= min && max <= high
        }
        (DeclKind::Choice(key), DeclKind::Choice(own)) => key
            .iter()
            .all(|option| own.iter().any(|o| o.eq_ignore_ascii_case(option))),
        (DeclKind::Bool, DeclKind::Bool) | (DeclKind::Payload, DeclKind::Payload) => true,
        _ => false,
    }
}

/// How an alias of kind `alias` takes values of the key's kind `first`:
/// whether it can at all, and for a choice, the key's options by the
/// alias's positions. Allocates: control thread, as the development builds.
fn reach(first: DeclKind, alias: DeclKind) -> (Option<DeclKind>, Box<[Option<u32>]>) {
    match (first, alias) {
        (DeclKind::Choice(key), DeclKind::Choice(own)) => {
            let positions = key.iter().map(|option| {
                let position = own.iter().position(|o| o.eq_ignore_ascii_case(option));
                position.map(|position| position as u32)
            });
            (Some(alias), positions.collect())
        }
        (DeclKind::Payload, _) | (_, DeclKind::Payload) => (None, Box::new([])),
        _ => (Some(alias), Box::new([])),
    }
}

impl DevelopmentControls {
    /// Declares every exposed key with a declared alias, or `None` when no
    /// alias is declared. Also returns the development surface's declared
    /// part, sharing the cells. Fails when a later declared alias of a key
    /// would refuse a value its first takes (see the module docs).
    pub(super) fn new(
        definition: &Invention,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
        module_indexes: &HashMap<String, usize>,
    ) -> Result<Option<(Self, DeclaredSurface)>, String> {
        let mut decls: Vec<ControlDecl> = Vec::new();
        let mut current = Vec::new();
        let mut aliases: Vec<Vec<Alias>> = Vec::new();
        // Each key's first declared alias, for the build error.
        let mut firsts: Vec<String> = Vec::new();
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
            let position = match decls.iter().position(|decl| decl.key == control.name) {
                Some(position) => position,
                None => {
                    decls.push(ControlDecl {
                        key: Cow::Owned(control.name.clone()),
                        indexed: false,
                        count: 1,
                        // Each alias clamps for itself as it applies; an
                        // integer keeps its clamp, so the key takes every
                        // whole number its first alias does.
                        clamp: match found.decl.kind {
                            DeclKind::Integer { .. } => found.decl.clamp,
                            _ => None,
                        },
                        ..found.decl.clone()
                    });
                    current.push(found.current);
                    aliases.push(Vec::new());
                    firsts.push(format!("{}.{}", control.module, control.control));
                    decls.len() - 1
                }
            };
            let first = &decls[position];
            if found.decl.writer != first.writer || !takes_all(first, &found.decl) {
                return Err(format!(
                    "Development control '{}': '{}.{}' does not take every value '{}' does; \
                     the aliases of one key must, with the one taking the fewest listed first",
                    control.name, control.module, control.control, firsts[position]
                ));
            }
            // One event alias makes the key an event: never coalesced.
            decls[position].event |= found.decl.event;
            let (kind, choices) = reach(decls[position].kind, found.decl.kind);
            aliases[position].push(Alias {
                module,
                index: found.index,
                kind,
                clamp: found.decl.clamp,
                choices,
            });
        }
        if decls.is_empty() {
            return Ok(None);
        }
        let table =
            ControlTable::built(decls).map_err(|why| format!("Development controls: {why}"))?;
        let cells = Arc::new(ControlCells::new(current));
        let surface = DeclaredSurface::fanning_out(table.clone(), cells.clone());
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

    /// Applies `value` to every declared alias of `control` in turn, each
    /// as its own kind holds it, publishing each inner control's new value.
    /// Returns what the first alias then holds. A value an alias cannot
    /// hold, or an alias whose module is gone, is refused before any alias
    /// changes; past that, an alias's own refusal (a module refusing what
    /// its declaration takes) is returned, and every other alias still
    /// applies. Audio thread: allocation-, free- and lock-free as long as
    /// the inner modules' `apply` are.
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
        for alias in aliases {
            if alias.module >= modules.len() {
                return Err(Refusal::TargetGone);
            }
            if alias
                .kind
                .and_then(|kind| convert(value, kind, alias.clamp, &alias.choices))
                .is_none()
            {
                return Err(Refusal::Invalid);
            }
        }
        let mut first = None;
        let mut refused = None;
        for alias in aliases {
            let Some(module) = modules.get_mut(alias.module) else {
                refused.get_or_insert(Refusal::TargetGone);
                continue;
            };
            let module = module.module_mut();
            // Its own automation still waiting was written before this.
            take_automation(module);
            let converted = alias
                .kind
                .and_then(|kind| convert(value, kind, alias.clamp, &alias.choices));
            let result = converted.ok_or(Refusal::Invalid).and_then(|value| {
                apply_declared(module, alias.index, value)?;
                let (_, cells) = module.declared().ok_or(Refusal::Unsupported)?;
                cells.load(alias.index).ok_or(Refusal::Unsupported)
            });
            if let Err(refusal) = result {
                refused.get_or_insert(refusal);
            }
            first.get_or_insert(result);
        }
        match (refused, first) {
            (Some(refusal), _) => Err(refusal),
            (None, Some(applied)) => applied,
            (None, None) => Err(Refusal::Unsupported),
        }
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
