//! A development instance's starting values, built into its inner modules.
//!
//! An instance's config sets its exposed controls. Written as controls
//! after the build, a value an inner module glides to (a vca's level, a
//! gain's decibels) would glide there from the module's default as the
//! first block plays, rather than start there. So each value is first
//! recorded into the config of the inner modules its control reaches, as
//! an authored control write records it (see
//! [`authored_document::write_control`]), since a document rebuilds to the
//! sound such a write made: the modules are built at their starting values,
//! and the writes after the build find them already there.
//!
//! An inner module reads a folded value as it reads its own config, which
//! for a declared control is the live write's own coercion. When one
//! refuses a value as config, every value is asked of the modules it
//! reaches as a live write to them would be (each built on its own, at the
//! definition's config): a value one of them refuses is left out, and the
//! rest start in place, as each coerced it. The writes after the build
//! then report the one left out (see `apply_development_config`).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::ranges::{self, Ranges};
use super::{DevelopmentFactory, DevelopmentModule, InventionBuilder};
use crate::invention::authored_document;
use crate::invention::load_warnings::{self, Warning};
use crate::invention::runtime::ControlSurfaceInstance;
use crate::traits::exact_number::json_scalar;
use crate::{ControlValue, Invention};

/// What a value folds as into one alias's module (`module`, `control`):
/// the value to write, or `None` when that module refuses it.
type Fold<'a> = &'a dyn Fn(&str, &str, ControlValue) -> Option<ControlValue>;

/// Builds `factory`'s development with its inner modules at the starting
/// values in `config`, building its definition once. An event (a key an
/// alias fires) is no value to start at, so it is never folded: an inner
/// module may read its key as config (a sample player's `play` as its
/// start state), or fire it as it builds (see [`event_names`]). A value
/// an inner module refuses as config is left out, as its live write would
/// be refused, and the rest start in place, in a second build (see the
/// module docs). A definition that still refuses the values, or that no longer
/// has every exposed control (a count written below a position another key
/// reaches), builds as written: the writes after the build then apply, or
/// report what is wrong, as they always did. Returns what the kept build
/// reported, and the keys of `config` the kept build started its inner
/// modules at.
pub(super) fn build_started(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    config: &Value,
) -> Result<(Built, Vec<String>), Box<dyn Error>> {
    let assemble = |definition: Invention| assemble(factory, sample_rate, definition);
    let as_written = || assemble(factory.definition.clone()).map(|built| (built, Vec::new()));
    let events = event_names(factory, sample_rate);
    let skip: Vec<&str> = (config.as_object().into_iter().flatten())
        .map(|(key, _)| key.as_str())
        .filter(|key| events.contains(*key))
        .collect();
    let as_given: Fold = &|_, _, value| Some(value);
    let Some((started, folded)) = with_starting_values(factory, config, &skip, as_given) else {
        return as_written();
    };
    if let Ok(built) = assemble(started) {
        return Ok((built, folded));
    }
    let accepted = accepted_by_reached(factory, sample_rate, config);
    let accepted: Fold = &|module, control, value| accepted(module, control, value);
    match with_starting_values(factory, config, &skip, accepted) {
        // The values each module takes, when they differ from those it
        // refused, may build where those did not.
        Some((started, folded)) if !refuses_alike(&started, factory, config, &skip) => {
            match assemble(started) {
                Ok(built) => Ok((built, folded)),
                Err(_) => as_written(),
            }
        }
        _ => as_written(),
    }
}

/// Whether `started` folds exactly what the first build folded, which its
/// definition refused: no value was left out or coerced.
fn refuses_alike(
    started: &Invention,
    factory: &DevelopmentFactory,
    config: &Value,
    skip: &[&str],
) -> bool {
    let as_given: Fold = &|_, _, value| Some(value);
    with_starting_values(factory, config, skip, as_given).is_some_and(|(given, _)| {
        (given.modules.iter().zip(&started.modules)).all(|(a, b)| a.config == b.config)
    })
}

/// The exposed controls of `factory`'s development an alias of which fires
/// an event, found once per factory without building its definition: an
/// alias into a nested development asks that development's factory, and
/// one into any other module asks that module, built on its own at the
/// definition's config to validate, as a document is before it loads. A
/// module that cannot be built on its own is taken to fire none; its build
/// then reports what is wrong.
pub(super) fn event_names(factory: &DevelopmentFactory, sample_rate: u32) -> &HashSet<String> {
    factory
        .events
        .get_or_init(|| find_events(factory, sample_rate))
}

fn find_events(factory: &DevelopmentFactory, sample_rate: u32) -> HashSet<String> {
    let definition = &factory.definition;
    let mut events = HashSet::new();
    if definition.controls.is_empty() {
        return events;
    }
    // The registry its modules build from: its own, unless it declares
    // developments of its own.
    let scoped;
    let registry = match definition.developments.is_empty() {
        true => &factory.registry,
        false => {
            let registered = factory.registered.lock().unwrap().clone();
            let built = InventionBuilder::with_registry_and_registered(
                sample_rate,
                factory.registry.for_validation(),
                Arc::new(Mutex::new(registered)),
                factory.loaded.clone(),
            )
            .register_developments_only(definition);
            let Ok(built) = built else {
                return events;
            };
            scoped = built;
            &scoped
        }
    };
    let mut probes: HashMap<&str, Option<ControlSurfaceInstance>> = HashMap::new();
    for alias in &definition.controls {
        let Some(spec) = (definition.modules.iter()).find(|spec| spec.id == alias.module) else {
            continue;
        };
        let fires = match registry.development(&spec.module_type) {
            Some(nested) => event_names(nested, sample_rate).contains(&alias.control),
            None => probes
                .entry(spec.id.as_str())
                .or_insert_with(|| {
                    let (built, _) = load_warnings::collected(|| {
                        registry.probe(&spec.module_type, sample_rate, &spec.config)
                    });
                    built.and_then(|built| built.control_surface)
                })
                .as_ref()
                .is_some_and(|surface| {
                    (surface.declaration(&alias.control)).is_some_and(|found| found.decl.event)
                }),
        };
        if fires {
            events.insert(alias.name.clone());
        }
    }
    events
}

#[cfg(test)]
thread_local! {
    /// How many times this thread has built a development's definition.
    pub(super) static ASSEMBLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A development built, with its surface and what its build reported (see
/// [`load_warnings`]).
pub(super) type Built = (DevelopmentModule, ControlSurfaceInstance, Vec<Warning>);

/// `factory`'s development built from `definition`, one of its own.
fn assemble(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    definition: Invention,
) -> Result<Built, Box<dyn Error>> {
    let builder = InventionBuilder::with_registry_and_registered(
        sample_rate,
        factory.registry.clone(),
        factory.registered.clone(),
        factory.loaded.clone(),
    );
    // Collected so the inner build keeps its warnings for this instance to
    // report, naming itself, rather than printing them first.
    #[cfg(test)]
    ASSEMBLED.with(|count| count.set(count.get() + 1));
    let (built, _) = load_warnings::collected(|| builder.build(definition));
    let (runtime, _handles) = built?;
    let warnings = std::mem::take(&mut runtime.state.lock().unwrap().load_warnings);
    let (module, surface) = DevelopmentModule::new(&factory.name, runtime, &factory.definition)?;
    Ok((module, surface, warnings))
}

/// What each value of `config` folds as into each inner module it reaches,
/// asked of that module as a live write to it would be: built on its own at
/// the definition's config, it coerces the value and takes it as a write,
/// or refuses it. An event folds as nothing, as no value to start at. A
/// module that cannot be built on its own takes each value as given; its
/// build then reports what is wrong.
fn accepted_by_reached(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    config: &Value,
) -> impl Fn(&str, &str, ControlValue) -> Option<ControlValue> {
    let definition = &factory.definition;
    let configured = config.as_object();
    let reached: HashSet<&str> = (definition.controls.iter())
        .filter(|alias| configured.is_some_and(|map| map.contains_key(&alias.name)))
        .map(|alias| alias.module.as_str())
        .collect();
    // Built to validate, as a document is before it loads: nothing plays,
    // records or reports, and the developments it declares are registered.
    let registered = factory.registered.lock().unwrap().clone();
    let registry = InventionBuilder::with_registry_and_registered(
        sample_rate,
        factory.registry.for_validation(),
        Arc::new(Mutex::new(registered)),
        factory.loaded.clone(),
    )
    .register_developments_only(definition);
    let mut surfaces: HashMap<String, ControlSurfaceInstance> = HashMap::new();
    if let Ok(registry) = registry {
        for spec in (definition.modules.iter()).filter(|spec| reached.contains(spec.id.as_str())) {
            let (built, _) = load_warnings::collected(|| {
                registry.build(&spec.module_type, sample_rate, &spec.config)
            });
            if let Some(surface) = built.ok().and_then(|built| built.control_surface) {
                surfaces.insert(spec.id.clone(), surface);
            }
        }
    }
    move |module: &str, control: &str, value: ControlValue| {
        let Some(surface) = surfaces.get(module) else {
            return Some(value);
        };
        if (surface.declaration(control)).is_some_and(|found| found.decl.event) {
            return None;
        }
        let value = surface.coerce_value(control, value);
        let (taken, _) = load_warnings::collected(|| surface.set_control(control, value.clone()));
        taken.ok().map(|()| value)
    }
}

/// An instance's `config` with a starting value for each control that
/// declares a range its definition starts any of its aliases outside, and
/// that config leaves out: the first alias's start, held within
/// the range, as a configured value is (written to every alias, as a
/// configured one is). `config` as is when there is none.
pub(super) fn with_held_starts<'a>(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    config: &'a Value,
) -> Cow<'a, Value> {
    let empty = serde_json::Map::new();
    let configured = match config {
        Value::Null => &empty,
        Value::Object(map) => map,
        _ => return Cow::Borrowed(config),
    };
    // Found once, but never from a build that failed: a later one may not.
    let held = match factory.held_starts.get() {
        Some(held) => held,
        None => match held_starts(factory, sample_rate) {
            Some(found) => factory.held_starts.get_or_init(|| found),
            None => return Cow::Borrowed(config),
        },
    };
    let mut missing = (held.iter())
        .filter(|(name, _)| !configured.contains_key(name))
        .peekable();
    if missing.peek().is_none() {
        return Cow::Borrowed(config);
    }
    let mut with_held = configured.clone();
    for (name, start) in missing {
        with_held.insert(name.clone(), Value::from(f64::from(*start)));
    }
    Cow::Owned(Value::Object(with_held))
}

/// Each control `factory`'s definition declares a range for and starts an
/// alias of outside it, with its first alias's start held within it. Builds
/// the definition only when it declares a range; a definition that fails to
/// build has nothing held (`None`), and its build reports why.
fn held_starts(factory: &DevelopmentFactory, sample_rate: u32) -> Option<Vec<(String, f32)>> {
    let definition = &factory.definition;
    let declared = Ranges::of(&factory.name, definition);
    let mut ranged: Vec<(&str, (f32, f32))> = Vec::new();
    let mut seen = HashSet::new();
    for control in &definition.controls {
        if let Ok(Some(range)) = declared.get(&control.name) {
            if seen.insert(control.name.as_str()) {
                ranged.push((&control.name, range));
            }
        }
    }
    if ranged.is_empty() {
        return Some(Vec::new());
    }
    let builder = InventionBuilder::with_registry_and_registered(
        sample_rate,
        factory.registry.clone(),
        factory.registered.clone(),
        factory.loaded.clone(),
    );
    let (runtime, _handles) = builder.build(definition.clone()).ok()?;
    let surfaces = runtime.control_surfaces.lock().unwrap().clone();
    let mut held = Vec::new();
    for (name, (min, max)) in ranged {
        let starts: Vec<f32> = (definition.controls.iter())
            .filter(|control| control.name == name)
            .filter_map(|control| {
                let start = surfaces.get(&control.module)?.get_control(&control.control);
                start.ok()?.as_number().ok()
            })
            .collect();
        let outside = starts.iter().any(|start| !(min..=max).contains(start));
        if let (true, Some(first)) = (outside, starts.first()) {
            held.push((name.to_string(), first.max(min).min(max)));
        }
    }
    Some(held)
}

/// `factory`'s definition with each scalar value of an instance's `config`,
/// but those of the keys in `skip`, written into the config of every inner
/// module its control reaches, under that control's key, as `fold` has
/// that module take it, with the keys written that no other key of
/// `config` shares a target with (a shared target holds whichever is
/// written last, which the writes after the build decide); or `None` when
/// no value reaches one. A value one of its modules refuses is written to none of
/// them, as a live write to its control changes none of them.
fn with_starting_values(
    factory: &DevelopmentFactory,
    config: &Value,
    skip: &[&str],
    fold: Fold,
) -> Option<(Invention, Vec<String>)> {
    let (development, definition) = (&factory.name, &factory.definition);
    let values = config.as_object()?;
    let declared = Ranges::of(development, definition);
    let mut built = definition.clone();
    let mut folded = Vec::new();
    for (name, value) in values
        .iter()
        .filter(|(name, _)| !skip.contains(&name.as_str()))
    {
        let Some(value) = json_scalar(value) else {
            continue;
        };
        // Held within the range its control declares, as a write to it is;
        // a range the build refuses is reported by the build.
        let range = declared.get(name).ok().flatten();
        let value = ranges::held(value, range);
        let aliases = (definition.controls.iter()).filter(|alias| alias.name == *name);
        let folds: Option<Vec<_>> = aliases
            .map(|alias| {
                let folded = fold(&alias.module, &alias.control, value.clone())?;
                Some((alias, folded))
            })
            .collect();
        for (alias, value) in folds.into_iter().flatten() {
            let keys_of = |module_type: &str| factory.registry.config_keys(module_type);
            authored_document::write_control(
                &mut built,
                &alias.module,
                &alias.control,
                &value,
                keys_of,
            );
            if folded.last() != Some(name) {
                folded.push(name.clone());
            }
        }
    }
    if folded.is_empty() {
        return None;
    }
    let reaches = |name: &str, module: &str, control: &str| {
        (definition.controls.iter())
            .any(|alias| alias.name == name && alias.module == module && alias.control == control)
    };
    folded.retain(|name| {
        let shared = |alias: &&crate::invention::DevelopmentControl| {
            values
                .keys()
                .any(|other| other != name && reaches(other, &alias.module, &alias.control))
        };
        !(definition.controls.iter()).any(|alias| alias.name == *name && shared(&alias))
    });
    Some((built, folded))
}
