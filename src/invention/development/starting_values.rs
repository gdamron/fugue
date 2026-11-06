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

use std::borrow::Cow;
use std::error::Error;

use serde_json::Value;

use super::{ranges, DevelopmentFactory, DevelopmentModule, InventionBuilder};
use crate::invention::authored_document;
use crate::invention::runtime::ControlSurfaceInstance;
use crate::traits::exact_number::json_scalar;
use crate::{Invention, ModuleRegistry};

/// Builds `factory`'s development with its inner modules at the starting
/// values in `config`. An event (a key an alias fires) is no value to start
/// at, so a build that met one builds again without it. A definition whose
/// modules refuse the values as config, or that no longer has every
/// exposed control (a count written below a position another key reaches),
/// builds as written: the writes after the build then apply, or report
/// what is wrong, as they always did.
pub(super) fn build_started(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    config: &Value,
) -> Result<(DevelopmentModule, ControlSurfaceInstance), Box<dyn Error>> {
    let assemble = |definition: Invention| assemble(factory, sample_rate, definition);
    let (definition, registry) = (&factory.definition, &factory.registry);
    let Some(started) = with_starting_values(&factory.name, definition, config, registry, &[])
    else {
        return assemble(definition.clone());
    };
    let Ok((module, surface)) = assemble(started) else {
        return assemble(definition.clone());
    };
    let events: Vec<&str> = (config.as_object().into_iter().flatten())
        .map(|(key, _)| key.as_str())
        .filter(|key| {
            surface
                .declaration(key)
                .is_some_and(|found| found.decl.event)
        })
        .collect();
    if events.is_empty() {
        return Ok((module, surface));
    }
    let without = with_starting_values(&factory.name, definition, config, registry, &events);
    without
        .and_then(|started| assemble(started).ok())
        .map_or_else(|| assemble(definition.clone()), Ok)
}

/// `factory`'s development built from `definition`, one of its own.
fn assemble(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    definition: Invention,
) -> Result<(DevelopmentModule, ControlSurfaceInstance), Box<dyn Error>> {
    let builder = InventionBuilder::with_registry_and_registered(
        sample_rate,
        factory.registry.clone(),
        factory.registered.clone(),
        factory.loaded.clone(),
    );
    let (runtime, _handles) = builder.build(definition)?;
    DevelopmentModule::new(&factory.name, runtime, &factory.definition)
}

/// An instance's `config` with a starting value for each control that
/// declares a range its definition starts any of its aliases outside, and
/// that config gives no scalar value: the first alias's start, held within
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
    let held = (factory.held_starts).get_or_init(|| held_starts(factory, sample_rate));
    let mut missing = (held.iter())
        .filter(|(name, _)| configured.get(name).and_then(json_scalar).is_none())
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
/// build has nothing held, and its build reports why.
fn held_starts(factory: &DevelopmentFactory, sample_rate: u32) -> Vec<(String, f32)> {
    let definition = &factory.definition;
    let mut ranged: Vec<(&str, (f32, f32))> = Vec::new();
    for control in &definition.controls {
        let range = ranges::declared(&factory.name, definition, &control.name);
        if let Ok(Some(range)) = range {
            if !ranged.iter().any(|(name, _)| *name == control.name) {
                ranged.push((&control.name, range));
            }
        }
    }
    if ranged.is_empty() {
        return Vec::new();
    }
    let builder = InventionBuilder::with_registry_and_registered(
        sample_rate,
        factory.registry.clone(),
        factory.registered.clone(),
        factory.loaded.clone(),
    );
    let Ok((runtime, _handles)) = builder.build(definition.clone()) else {
        return Vec::new();
    };
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
    held
}

/// `definition` with each scalar value of an instance's `config`, but those
/// of the keys in `skip`, written into the config of every inner module its
/// control reaches, under that control's key; or `None` when no value
/// reaches one.
fn with_starting_values(
    development: &str,
    definition: &Invention,
    config: &Value,
    registry: &ModuleRegistry,
    skip: &[&str],
) -> Option<Invention> {
    let values = config.as_object()?;
    let mut built = definition.clone();
    let mut written = false;
    for (name, value) in values
        .iter()
        .filter(|(name, _)| !skip.contains(&name.as_str()))
    {
        let Some(value) = json_scalar(value) else {
            continue;
        };
        // Held within the range its control declares, as a write to it is;
        // a range the build refuses is reported by the build.
        let range = ranges::declared(development, definition, name)
            .ok()
            .flatten();
        let value = ranges::held(value, range);
        for alias in definition
            .controls
            .iter()
            .filter(|alias| alias.name == *name)
        {
            let keys_of = |module_type: &str| registry.config_keys(module_type);
            authored_document::write_control(
                &mut built,
                &alias.module,
                &alias.control,
                &value,
                keys_of,
            );
            written = true;
        }
    }
    written.then_some(built)
}
