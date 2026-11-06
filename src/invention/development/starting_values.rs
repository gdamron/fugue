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
use crate::{ControlValue, Invention, ModuleRegistry};

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
/// declares a range its definition starts it outside (a value config
/// leaves out): that start, held within the range, as a configured one
/// is. `config` as is when no such control exists, which takes no build
/// for a development declaring no range.
pub(super) fn with_held_starts<'a>(
    factory: &DevelopmentFactory,
    sample_rate: u32,
    config: &'a Value,
) -> Result<Cow<'a, Value>, Box<dyn Error>> {
    let definition = &factory.definition;
    let ranged: Vec<(&str, (f32, f32))> = (definition.controls.iter())
        .filter(|control| config.get(&control.name).is_none())
        .filter_map(|control| {
            let range = ranges::declared(&factory.name, definition, &control.name);
            Some((control.name.as_str(), range.ok().flatten()?))
        })
        .collect();
    let empty = serde_json::Map::new();
    let configured = match config {
        Value::Null => &empty,
        Value::Object(map) => map,
        _ => return Ok(Cow::Borrowed(config)),
    };
    if ranged.is_empty() {
        return Ok(Cow::Borrowed(config));
    }
    let (_, surface) = assemble(factory, sample_rate, definition.clone())?;
    let mut held = configured.clone();
    for (name, range) in ranged {
        if let Ok(ControlValue::Number(start)) = surface.get_control(name) {
            if let ControlValue::Number(within) =
                ranges::held(ControlValue::Number(start), Some(range))
            {
                if within != start {
                    held.insert(name.to_string(), Value::from(f64::from(within)));
                }
            }
        }
    }
    Ok(match held.len() == configured.len() {
        true => Cow::Borrowed(config),
        false => Cow::Owned(Value::Object(held)),
    })
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
