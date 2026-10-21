use std::sync::Arc;

use serde_json::{json, Value};

use super::{LegacyRefusal, LEGACY_CONTROLS};
use crate::alloc_counter::allocator_events;
use crate::control_request::table::{TableTarget, TableValue};
use crate::control_request::EventCounter;
use crate::invention::runtime::ControlSurfaceInstance;
use crate::module_config::tests::registry::{base_config, eight_frame_wav, UNBUILDABLE};
use crate::ControlValue;

/// Every built-in type with controls either declares them or is listed,
/// never both; every type builds (from the registry harness's config)
/// unless it cannot be built in a test at all, and every listed type
/// is checked.
#[test]
fn a_module_type_declares_its_controls_or_is_listed_legacy() {
    let registry = crate::ModuleRegistry::default();
    let mut checked = Vec::new();
    for type_id in registry.types() {
        if registry.is_sink(type_id) || UNBUILDABLE.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let mut config = base_config(type_id);
        if type_id == "sample_slicer" {
            // Its controls exist only in elastic mode.
            config["mode"] = serde_json::json!("elastic");
        }
        let built = registry.build(type_id, 48_000, &config);
        let result = built.unwrap_or_else(|error| panic!("'{type_id}' did not build: {error}"));
        if result.control_surface.is_none() {
            continue;
        }
        let declared = result.module.module().declared().is_some();
        let listed = LEGACY_CONTROLS.contains(&type_id);
        assert!(
            declared != listed,
            "'{type_id}': declared {declared}, listed {listed}"
        );
        checked.push(type_id);
    }
    for listed in LEGACY_CONTROLS {
        assert!(
            checked.contains(listed),
            "'{listed}' was not checked: {checked:?}"
        );
    }
    for expected in ["oscillator", "vca"] {
        assert!(
            checked.contains(&expected),
            "'{expected}' not checked: {checked:?}"
        );
    }
}

/// The config a listed type is built from: every control it has a schedule
/// could write (a sample in each slot, zone and player, elastic mode).
fn scheduled_config(type_id: &str) -> Value {
    let wav = eight_frame_wav();
    match type_id {
        "sample_kit" => json!({ "samples": [{ "key": 36, "asset": wav }] }),
        "sample_instrument" => json!({
            "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": wav }]
        }),
        "sample_player" => json!({ "asset": wav, "mode": "elastic" }),
        "sample_slicer" => {
            let mut config = base_config(type_id);
            config["mode"] = json!("elastic");
            config
        }
        _ => base_config(type_id),
    }
}

/// A fresh surface of listed type `type_id`, with every control a
/// schedule could write.
pub(crate) fn scheduled_surface(type_id: &str) -> ControlSurfaceInstance {
    let registry = crate::ModuleRegistry::default();
    let built = registry.build(type_id, 48_000, &scheduled_config(type_id));
    let built = built.unwrap_or_else(|error| panic!("'{type_id}' did not build: {error}"));
    built.control_surface.expect("a listed type has controls")
}

/// The controls a schedule can reach (`resolve_schedule` takes those
/// holding a number or a boolean), with what each holds.
fn schedulable(surface: &ControlSurfaceInstance) -> Vec<(String, ControlValue)> {
    (surface.controls().into_iter())
        .filter_map(|meta| {
            let value = surface.get_control(&meta.key).ok()?;
            matches!(value, ControlValue::Number(_) | ControlValue::Bool(_))
                .then_some((meta.key, value))
        })
        .collect()
}

/// What a schedule may write a control holding `value`: values in and
/// past its range, and one of the other kind.
fn writes(value: &ControlValue) -> Vec<TableValue> {
    let numbers = |current: f32| {
        [
            current,
            current + 0.37,
            0.0,
            0.6,
            3.0,
            1000.0,
            -5.0,
            f32::INFINITY,
        ]
        .map(TableValue::Number)
    };
    match value {
        ControlValue::Number(current) => numbers(*current)
            .into_iter()
            .chain([TableValue::Bool(true)])
            .collect(),
        _ => vec![
            TableValue::Bool(true),
            TableValue::Bool(false),
            TableValue::Number(1.0),
        ],
    }
}

/// Every listed type's scheduled reads and writes, of every control a
/// schedule can reach and of keys it has not got, allocate and free
/// nothing: what is refused is counted, never formatted.
#[test]
fn a_scheduled_write_to_a_legacy_control_allocates_nothing() {
    for type_id in LEGACY_CONTROLS {
        let surface = scheduled_surface(type_id);
        let reached = schedulable(&surface);
        assert!(!reached.is_empty(), "'{type_id}' has schedulable controls");
        let missing = ["no_such_control".to_string(), format!("{}9", reached[0].0)];
        let targets = (reached
            .iter()
            .map(|(key, value)| (key.clone(), writes(value))))
        .chain(missing.map(|key| (key, writes(&ControlValue::Number(0.0)))))
        .map(|(key, values)| {
            let refused = Arc::new(EventCounter::new());
            (TableTarget::legacy(surface.clone(), key, refused), values)
        })
        .collect::<Vec<_>>();
        for (target, values) in &targets {
            let (_, allocs, frees) = allocator_events(|| {
                let _ = target.current();
                for &value in values {
                    assert!(target.write(value), "a legacy write ends no ramp");
                }
            });
            let TableTarget::Legacy { control, .. } = target else {
                unreachable!()
            };
            assert_eq!((allocs, frees), (0, 0), "'{type_id}.{control}'");
        }
    }
}

/// A scheduled write lands as the control thread's setter would: the same
/// success, and every control reading back the same, whatever is written.
#[test]
fn a_scheduled_write_lands_as_the_setter_would() {
    for type_id in LEGACY_CONTROLS {
        let (set, scheduled) = (scheduled_surface(type_id), scheduled_surface(type_id));
        for (key, value) in schedulable(&set) {
            for write in writes(&value) {
                let as_control = match write {
                    TableValue::Number(number) => ControlValue::Number(number),
                    TableValue::Bool(flag) => ControlValue::Bool(flag),
                };
                let expected = set.set_control(&key, as_control).is_ok();
                let landed = scheduled.set_scheduled(&key, write);
                assert_eq!(landed.is_ok(), expected, "'{type_id}.{key}' = {write:?}");
                for (shown, _) in schedulable(&set) {
                    let read = (set.get_control(&shown), scheduled.get_control(&shown));
                    assert_eq!(
                        read.0, read.1,
                        "'{type_id}.{shown}' after {key} = {write:?}"
                    );
                    let number = match read.0 {
                        Ok(ControlValue::Number(number)) => Ok(number),
                        _ => Err(LegacyRefusal::Unsupported),
                    };
                    assert_eq!(
                        scheduled.get_scheduled(&shown),
                        number,
                        "'{type_id}.{shown}'"
                    );
                }
            }
        }
    }
}

/// A refused write is counted, and the ramp it belongs to goes on.
#[test]
fn a_refused_write_is_counted_and_ends_no_ramp() {
    let surface = scheduled_surface("adsr");
    let refused = Arc::new(EventCounter::new());
    let target = TableTarget::legacy(surface, "no_such_control".into(), refused.clone());
    assert_eq!(target.current(), None);
    assert_eq!(
        refused.count(),
        0,
        "a control holding no number is not a refusal"
    );
    assert!(target.write(TableValue::Number(1.0)));
    assert!(target.write(TableValue::Bool(true)));
    assert_eq!(refused.count(), 2);
    assert!(Arc::ptr_eq(target.refused().unwrap(), &refused));
}
