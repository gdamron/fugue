use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::schedule::{parse_schedule_json, SurfaceMap};
use super::*;
use crate::modules::cell_sequencer::CellSequencerControls;
use crate::modules::mixer::MixerControls;
use crate::ControlSurface;

/// Builds a scheduler attached to a directory containing a 2-channel mixer
/// (id `mixer`, all levels 1.0) and a cell sequencer (id `cells`).
fn setup(schedule_json: &str) -> (ControlScheduler, MixerControls, SurfaceDirectory) {
    let mixer = MixerControls::new(2);
    let cells = CellSequencerControls::new();
    let mut map: SurfaceMap = IndexMap::new();
    map.insert(
        "mixer".to_string(),
        Arc::new(mixer.clone()) as Arc<dyn ControlSurface + Send + Sync>,
    );
    map.insert(
        "cells".to_string(),
        Arc::new(cells) as Arc<dyn ControlSurface + Send + Sync>,
    );
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));

    let spec = parse_schedule_json(schedule_json).unwrap();
    let ctrl = ControlSchedulerControls::new(spec);
    ctrl.attach("sched", &directory).unwrap();
    let module = ControlScheduler::new(48_000, ctrl);
    (module, mixer, directory)
}

/// Sends one gate rising edge (one high frame, then `low_frames` low frames),
/// processing one frame at a time.
fn pulse(module: &mut ControlScheduler, low_frames: usize) {
    module.set_input("gate", 1.0).unwrap();
    module.process(1);
    module.set_input("gate", 0.0).unwrap();
    for _ in 0..low_frames {
        module.process(1);
    }
}

#[test]
fn jump_fires_on_exact_step() {
    let (mut module, mixer, _dir) =
        setup(r#"[{ "at": 2, "module": "mixer", "control": "level.0", "value": 0.25 }]"#);

    pulse(&mut module, 15); // step 0
    assert_eq!(mixer.level(0), 1.0);
    pulse(&mut module, 15); // step 1
    assert_eq!(mixer.level(0), 1.0);

    // The edge frame that begins step 2 applies the change immediately.
    module.set_input("gate", 1.0).unwrap();
    module.process(1);
    assert_eq!(mixer.level(0), 0.25);
    assert_eq!(module.get_output("step").unwrap(), 2.0);
}

#[test]
fn first_edge_is_step_zero() {
    let (mut module, mixer, _dir) =
        setup(r#"[{ "at": 0, "module": "mixer", "control": "level.1", "value": 0.5 }]"#);

    // Nothing fires before any gate arrives.
    for _ in 0..8 {
        module.process(1);
    }
    assert_eq!(mixer.level(1), 1.0);

    module.set_input("gate", 1.0).unwrap();
    module.process(1);
    assert_eq!(mixer.level(1), 0.5);
}

#[test]
fn ramp_hits_exact_boundary_values() {
    let (mut module, mixer, _dir) =
        setup(r#"[{ "at": 1, "module": "mixer", "control": "level.0", "value": 0.0, "ramp": 4 }]"#);

    let period = 16;
    pulse(&mut module, period - 1); // step 0
    pulse(&mut module, period - 1); // step 1: ramp starts from 1.0

    // Each subsequent boundary lands exactly on from + (to - from) * k / N.
    for k in 1..=4_u32 {
        module.set_input("gate", 1.0).unwrap();
        module.process(1);
        let expected = 1.0 + (0.0 - 1.0) * (k as f32 / 4.0);
        assert_eq!(
            mixer.level(0),
            expected,
            "boundary {} of the ramp must be exact",
            k
        );
        module.set_input("gate", 0.0).unwrap();
        // Between boundaries the ramp interpolates monotonically without
        // overshooting the next boundary value.
        let next = 1.0 + (0.0 - 1.0) * ((k as f32 + 1.0).min(4.0) / 4.0);
        let mut last = mixer.level(0);
        for _ in 0..period - 1 {
            module.process(1);
            let value = mixer.level(0);
            assert!(value <= last + f32::EPSILON, "ramp must not move backwards");
            assert!(value >= next - f32::EPSILON, "ramp must not overshoot");
            last = value;
        }
    }

    // The ramp is finished: further edges leave the value at its target.
    pulse(&mut module, period - 1);
    assert_eq!(mixer.level(0), 0.0);
}

#[test]
fn jump_cancels_conflicting_ramp() {
    let (mut module, mixer, _dir) = setup(
        r#"[
            { "at": 0, "module": "mixer", "control": "level.0", "value": 0.0, "ramp": 8 },
            { "at": 2, "module": "mixer", "control": "level.0", "value": 0.7 }
        ]"#,
    );

    pulse(&mut module, 15); // step 0: ramp starts
    pulse(&mut module, 15); // step 1: ramping down
    assert!(mixer.level(0) < 1.0);

    pulse(&mut module, 15); // step 2: jump supersedes the ramp
    assert_eq!(mixer.level(0), 0.7);
    pulse(&mut module, 15); // step 3: cancelled ramp writes nothing further
    assert_eq!(mixer.level(0), 0.7);
}

#[test]
fn reset_rearms_the_schedule() {
    let (mut module, mixer, _dir) =
        setup(r#"[{ "at": 0, "module": "mixer", "control": "level.0", "value": 0.5 }]"#);

    pulse(&mut module, 3);
    assert_eq!(mixer.level(0), 0.5);

    mixer.set_level(0, 1.0);
    module.set_input("reset", 1.0).unwrap();
    module.process(1);
    module.set_input("reset", 0.0).unwrap();
    assert_eq!(module.get_output("step").unwrap(), -1.0);

    pulse(&mut module, 3);
    assert_eq!(mixer.level(0), 0.5, "entry re-fires after reset");
}

#[test]
fn schedule_replaced_during_playback_skips_past_entries() {
    let (mut module, mixer, _dir) = setup("[]");
    let ctrl = module.controls().clone();

    pulse(&mut module, 3); // step 0
    pulse(&mut module, 3); // step 1

    ctrl.set_schedule_json(
        r#"[
            { "at": 1, "module": "mixer", "control": "level.0", "value": 0.1 },
            { "at": 3, "module": "mixer", "control": "level.1", "value": 0.3 }
        ]"#,
    )
    .unwrap();

    pulse(&mut module, 3); // step 2 (adoption happens here)
    pulse(&mut module, 3); // step 3
    assert_eq!(mixer.level(0), 1.0, "entry in the past must not fire");
    assert_eq!(mixer.level(1), 0.3, "future entry fires at its step");
}

#[test]
fn bool_controls_can_be_scheduled() {
    let (mut module, _mixer, dir) = setup(
        r#"[{ "at": 0, "module": "cells", "control": "wait_for_cycle_end", "value": true }]"#,
    );

    pulse(&mut module, 1);
    let cells = dir.lock().unwrap().get("cells").unwrap().clone();
    assert_eq!(
        cells.get_control("wait_for_cycle_end").unwrap(),
        crate::ControlValue::Bool(true)
    );
}

#[test]
fn control_targets_reports_unique_modules() {
    let (module, _mixer, _dir) = setup(
        r#"[
            { "at": 0, "module": "mixer", "control": "level.0", "value": 0.1 },
            { "at": 1, "module": "mixer", "control": "level.1", "value": 0.2 },
            { "at": 2, "module": "cells", "control": "wait_for_cycle_end", "value": true }
        ]"#,
    );
    assert_eq!(module.control_targets(), vec!["mixer", "cells"]);
}

#[test]
fn resolution_rejects_bad_schedules() {
    let cases = [
        (
            r#"[{ "at": 0, "module": "nope", "control": "level.0", "value": 0.1 }]"#,
            "unknown module",
        ),
        (
            r#"[{ "at": 0, "module": "mixer", "control": "nope", "value": 0.1 }]"#,
            "Unknown control",
        ),
        (
            r#"[{ "at": 0, "module": "mixer", "control": "level.0", "value": true }]"#,
            "does not match",
        ),
        (
            r#"[{ "at": 0, "module": "sched", "control": "step", "value": 0.0 }]"#,
            "cannot target itself",
        ),
        (
            r#"[{ "at": 0, "module": "cells", "control": "sequences_json", "value": 0.1 }]"#,
            "string control",
        ),
    ];
    for (json, needle) in cases {
        let (module, _mixer, _dir) = setup("[]");
        let err = module.controls().set_schedule_json(json).unwrap_err();
        assert!(err.contains(needle), "expected '{}' in '{}'", needle, err);
    }
}

#[test]
fn parsing_rejects_bad_entries() {
    let err = parse_schedule_json(
        r#"[{ "at": 0, "module": "m", "control": "c", "value": 0.1, "ramp": 0 }]"#,
    )
    .unwrap_err();
    assert!(err.contains("at least 1"), "{}", err);

    let err = parse_schedule_json(
        r#"[{ "at": 0, "module": "m", "control": "c", "value": true, "ramp": 2 }]"#,
    )
    .unwrap_err();
    assert!(err.contains("numeric"), "{}", err);

    let err = parse_schedule_json(r#"[{ "at": 0, "module": "m", "value": 0.1 }]"#).unwrap_err();
    assert!(err.contains("invalid schedule"), "{}", err);

    let err =
        parse_schedule_json(r#"[{ "at": 0, "module": "m", "control": "c", "value": "loud" }]"#)
            .unwrap_err();
    assert!(err.contains("invalid schedule"), "{}", err);
}

#[test]
fn schedule_control_round_trips_as_json() {
    let (module, _mixer, _dir) =
        setup(r#"[{ "at": 4, "module": "mixer", "control": "level.0", "value": 0.5, "ramp": 2 }]"#);
    let json = module.controls().schedule_json();
    let reparsed = parse_schedule_json(&json).unwrap();
    assert_eq!(reparsed.len(), 1);
    assert_eq!(reparsed[0].at, 4);
    assert_eq!(reparsed[0].ramp, Some(2));
}
