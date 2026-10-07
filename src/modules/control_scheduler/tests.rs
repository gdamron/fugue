use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::schedule::{parse_schedule, parse_schedule_json, SurfaceMap};
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
    // A live graph orders modules from the surface, so it must agree.
    assert_eq!(
        module.controls().control_targets(),
        module.control_targets()
    );
}

#[test]
fn attach_resolving_resolves_against_a_pending_directory() {
    let spec = parse_schedule_json(
        r#"[{ "at": 0, "module": "mixer", "control": "level.0", "value": 0.5 }]"#,
    )
    .unwrap();
    let ctrl = ControlSchedulerControls::new(spec);
    let directory: SurfaceDirectory = Arc::new(Mutex::new(IndexMap::new()));
    assert!(ctrl.attach("sched", &directory).is_err());

    // The mixer exists only in the directory a pending change will leave.
    let mut pending: SurfaceMap = IndexMap::new();
    pending.insert(
        "mixer".to_string(),
        Arc::new(MixerControls::new(2)) as Arc<dyn ControlSurface + Send + Sync>,
    );
    ctrl.attach_resolving("sched", &directory, &pending)
        .unwrap();
    assert_eq!(ctrl.control_targets(), vec!["mixer"]);
    assert!(directory.lock().unwrap().is_empty());
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
fn parsing_refuses_numbers_too_large_for_f32() {
    // JSON has no NaN or infinity, but 1e39 overflows an f32 to infinity,
    // which the audio thread would hand straight to the target's setter.
    for value in ["1e39", "-1e39"] {
        let json = format!(r#"[{{ "at": 2, "module": "m", "control": "c", "value": {value} }}]"#);
        let err = parse_schedule_json(&json).unwrap_err();
        assert!(
            err.contains("control 'm.c' expects a finite number"),
            "{}",
            err
        );
        let array: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            parse_schedule(&array).is_err(),
            "config form is refused too"
        );
    }

    // Finite extremes still parse.
    let json = format!(
        r#"[{{ "at": 0, "module": "m", "control": "c", "value": {} }}]"#,
        f32::MAX
    );
    assert!(parse_schedule_json(&json).is_ok());
}

#[test]
fn a_config_schedule_given_as_json_text_parses_like_the_array() {
    // The `schedule` control takes JSON text, and an authored control write
    // records that text in the config, so the config must build from it.
    let array = serde_json::json!([
        { "at": 4, "module": "mixer", "control": "level.0", "value": 0.5, "ramp": 2 }
    ]);
    let text = serde_json::Value::String(array.to_string());
    assert_eq!(
        parse_schedule(&text).unwrap(),
        parse_schedule(&array).unwrap()
    );
    assert!(parse_schedule(&serde_json::json!("[]")).unwrap().is_empty());

    let err = parse_schedule(&serde_json::json!("not json")).unwrap_err();
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

#[test]
fn a_prepared_scheduler_processes_its_first_block_without_allocating() {
    let schedule = r#"[
        { "at": 0, "module": "mixer", "control": "level.0", "value": 0.5 },
        { "at": 1, "module": "mixer", "control": "level.1", "value": 0.0, "ramp": 2 }
    ]"#;
    // Unprepared, the first block adopts the schedule on the audio thread.
    let (mut module, _mixer, _dir) = setup(schedule);
    let (_, allocs, _) = crate::alloc_counter::allocator_events(|| module.process(64));
    assert!(allocs > 0);

    let (mut module, mixer, _dir) = setup(schedule);
    module.prepare_for_publication();
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        pulse(&mut module, 15);
        pulse(&mut module, 15);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(mixer.level(0), 0.5);
}

/// A one-control surface (`raw.value`) that stores any number unclamped and
/// records every write, so a test can see exactly what a ramp writes.
struct RecordingSurface {
    writes: Mutex<Vec<f32>>,
    value: Mutex<f32>,
}

impl ControlSurface for RecordingSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![ControlMeta::number("value", "Unclamped number")]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "value" => Ok(ControlValue::Number(*self.value.lock().unwrap())),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match (key, value) {
            ("value", ControlValue::Number(number)) => {
                *self.value.lock().unwrap() = number;
                self.writes.lock().unwrap().push(number);
                Ok(())
            }
            _ => Err(format!("Unsupported write to '{}'", key)),
        }
    }
}

#[test]
fn ramp_between_extreme_values_writes_only_finite_monotonic_values() {
    // The FUG-302 repro: the control starts at f32::MIN and ramps to 3.4e38,
    // whose difference overflows f32.
    let surface = Arc::new(RecordingSurface {
        writes: Mutex::new(Vec::new()),
        value: Mutex::new(f32::MIN),
    });
    let mut map: SurfaceMap = IndexMap::new();
    map.insert(
        "raw".to_string(),
        surface.clone() as Arc<dyn ControlSurface + Send + Sync>,
    );
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));
    let spec = parse_schedule_json(
        r#"[{ "at": 0, "module": "raw", "control": "value", "value": 3.4e38, "ramp": 4 }]"#,
    )
    .unwrap();
    let ctrl = ControlSchedulerControls::new(spec);
    ctrl.attach("sched", &directory).unwrap();
    let mut module = ControlScheduler::new(48_000, ctrl);

    for _ in 0..6 {
        pulse(&mut module, 15);
    }

    let writes = surface.writes.lock().unwrap();
    assert!(writes.len() > 4, "the ramp must write between boundaries");
    for pair in writes.windows(2) {
        assert!(pair[1] >= pair[0], "ramp moved backwards: {:?}", pair);
    }
    for &value in writes.iter() {
        assert!(value.is_finite(), "ramp wrote a non-finite value");
        assert!((f32::MIN..=3.4e38).contains(&value));
    }
    assert_eq!(*writes.last().unwrap(), 3.4e38_f32);
}

#[test]
fn ramp_value_is_finite_and_monotonic_between_the_extremes() {
    for (from, to) in [(f32::MIN, f32::MAX), (f32::MAX, f32::MIN)] {
        assert_eq!(ramp_value(from, to, 0.0), from);
        assert_eq!(ramp_value(from, to, 1.0), to);
        let mut last = from;
        for i in 0..=1024 {
            let value = ramp_value(from, to, i as f32 / 1024.0);
            assert!(value.is_finite());
            assert!(value >= from.min(to) && value <= from.max(to));
            if to > from {
                assert!(value >= last);
            } else {
                assert!(value <= last);
            }
            last = value;
        }
    }
}

#[test]
fn ramp_value_matches_the_f32_formula_for_typical_ranges() {
    // Levels, pans, frequencies, decibels and tempos, in both directions.
    let ranges = [
        (0.0_f32, 1.0_f32),
        (1.0, 0.0),
        (0.2, 2.0),
        (-1.0, 1.0),
        (20.0, 20_000.0),
        (440.0, 220.0),
        (-60.0, 0.0),
        (60.0, 180.0),
        (120.0, 90.0),
    ];
    for (from, to) in ranges {
        let tolerance = 2.0 * f32::EPSILON * from.abs().max(to.abs());
        for i in 0..=1000 {
            let progress = i as f32 / 1000.0;
            let previous = from + (to - from) * progress;
            let value = ramp_value(from, to, progress);
            assert!(
                (value - previous).abs() <= tolerance,
                "{} -> {} at {}: {} vs {}",
                from,
                to,
                progress,
                value,
                previous
            );
        }
    }
}
