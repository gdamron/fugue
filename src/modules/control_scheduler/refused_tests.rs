//! A scheduler counts the legacy writes its targets refuse, for a status to
//! show, across every schedule it plays.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::schedule::{parse_schedule_json, SurfaceMap};
use super::*;
use crate::control_request::{LegacyRefusal, TableValue};
use crate::{ControlMeta, ControlSurface, ControlValue};

/// A one-control surface (`full.value`) whose scheduled writes are all
/// refused, as a full note queue refuses a note.
struct FullSurface;

impl ControlSurface for FullSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![ControlMeta::number("value", "Refused when scheduled")]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "value" => Ok(ControlValue::Number(0.0)),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, _value: ControlValue) -> Result<(), String> {
        Err(format!("Unsupported write to '{}'", key))
    }

    #[allow(private_interfaces)]
    fn set_scheduled(&self, _key: &str, _value: TableValue) -> Result<(), LegacyRefusal> {
        Err(LegacyRefusal::Full)
    }
}

const TWO_WRITES: &str = r#"[
    {"at_step": 0, "module": "full", "control": "value", "value": 1.0},
    {"at_step": 1, "module": "full", "control": "value", "value": 2.0}
]"#;

/// Sends one gate rising edge, then a low frame.
fn pulse(module: &mut ControlScheduler) {
    module.set_input("clock", 1.0).unwrap();
    module.process(1);
    module.set_input("clock", 0.0).unwrap();
    module.process(1);
}

#[test]
fn refused_writes_are_counted_across_schedules() {
    let mut map: SurfaceMap = IndexMap::new();
    map.insert(
        "full".to_string(),
        Arc::new(FullSurface) as Arc<dyn ControlSurface + Send + Sync>,
    );
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));
    let controls = ControlSchedulerControls::new(parse_schedule_json(TWO_WRITES).unwrap());
    controls.attach("sched", &directory).unwrap();
    let mut module = ControlScheduler::new(48_000, controls.clone());
    module.settle();
    module.settle();
    assert_eq!(controls.scheduled_writes_refused(), 0);

    pulse(&mut module);
    pulse(&mut module);
    assert_eq!(controls.scheduled_writes_refused(), 2);

    // A new schedule keeps counting where the last left off.
    let schedule = ControlValue::String(TWO_WRITES.to_string());
    controls.set_control("schedule", schedule).unwrap();
    module.settle();
    module.settle();
    module.set_input("reset", 1.0).unwrap();
    module.process(1);
    module.set_input("reset", 0.0).unwrap();
    pulse(&mut module);
    assert_eq!(controls.scheduled_writes_refused(), 3);
}
