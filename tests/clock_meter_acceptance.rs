//! Acceptance tests for the clock's meter, written from the spec alone.
//!
//! Spec summary: the clock has a whole-number `beats_per_bar` control and
//! config key (0 to 32, default 0 = no meter) and three read-only integer
//! telemetry controls updated per block: `bar` (from 1), `beat_in_bar` (from
//! 1) and `meter` (beats per bar in force for the current bar). All three
//! read 0 with no meter. A meter set on a clock with none counts bars from
//! the last reset (beat 0) at once; changing an existing meter is adopted at
//! the next bar line, and bars already played keep their numbers; a reset
//! returns to bar 1, beat 1 and adopts a pending meter at once. A meter write
//! can be timed like any control write. `ClockSummary` carries `bar`,
//! `beat_in_bar` and `beats_per_bar` (omitted when None), built by
//! `ClockSummary::read`. `NextBar` (control and edit timing) takes an
//! optional `beats_per_bar`: None lands on the clock's own next bar line,
//! with no meter it falls back to 4, and a given value counts multiples from
//! beat 0.
//!
//! The rig drives a live invention through a capturing backend, one 64-frame
//! block at a time, at 225 bpm (a beat is exactly 200 blocks at 48 kHz), and
//! samples at the middle of a beat so no check sits on a bar line.
//!
//! Run with `cargo test --test clock_meter_acceptance`.

use std::sync::{Arc, Mutex};

use fugue::rpc::{ClockSummary, ControlTiming, ControlWhen, EditWhen};
use fugue::{
    AudioBackend, ControlValue, ControlWrite, ControlWriteIntent, Invention, InventionBuilder,
    ModuleRegistry, RunningInvention,
};
use serde_json::{json, Value};

const RATE: u32 = 48_000;
const BPM: f64 = 225.0;

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

fn document(clock_config: Value) -> Value {
    let mut config = json!({ "bpm": BPM });
    for (key, value) in clock_config.as_object().into_iter().flatten() {
        config[key] = value.clone();
    }
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": config },
            { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    })
}

struct Rig {
    running: RunningInvention,
    slot: Arc<Mutex<Option<Render>>>,
}

/// What the clock reports at a moment: bar, beat in the bar, meter.
type Reading = (u32, u32, u32);

impl Rig {
    fn start(clock_config: Value) -> Rig {
        let invention: Invention = serde_json::from_value(document(clock_config)).unwrap();
        let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
        let slot = Arc::new(Mutex::new(None));
        let running = runtime
            .start_with_backend(Capture(slot.clone()))
            .expect("a capturing backend starts");
        let rig = Rig { running, slot };
        rig.block();
        rig
    }

    fn block(&self) {
        let mut slot = self.slot.lock().unwrap();
        let render = slot.as_mut().expect("backend started");
        render(&mut [0.0; 64], &mut [0.0; 64]);
    }

    fn number(&self, key: &str) -> f64 {
        match self.running.get_control("clock", key) {
            Ok(ControlValue::Number(n)) => f64::from(n),
            other => panic!("clock {key}: {other:?}"),
        }
    }

    fn position(&self) -> f64 {
        self.number("position")
    }

    /// Renders blocks until the clock has reached `beat` since its reset.
    fn run_to(&self, beat: f64) {
        for _ in 0..200_000 {
            if self.position() >= beat {
                return;
            }
            self.block();
        }
        panic!("the clock never reached beat {beat}");
    }

    fn set(&self, key: &str, value: f32) {
        self.running
            .set_control("clock", key, ControlValue::Number(value))
            .unwrap_or_else(|e| panic!("set {key}: {e:?}"));
    }

    fn summary(&self) -> ClockSummary {
        ClockSummary::read("clock", |key| self.running.get_control("clock", key).ok())
    }

    fn reading(&self) -> Reading {
        (
            self.number("bar") as u32,
            self.number("beat_in_bar") as u32,
            self.number("meter") as u32,
        )
    }

    /// A write of the oscillator's frequency at `when`.
    fn timed_frequency(&self, hz: f32, when: ControlWhen) {
        let write = ControlWrite {
            intent: ControlWriteIntent::Perform,
            timing: ControlTiming {
                when: Some(when),
                ttl_ms: None,
            },
            ..ControlWrite::new("osc", "frequency", ControlValue::Number(hz))
        };
        let acks = self.running.write_controls(&[write]);
        assert!(acks.error.is_none(), "{:?}", acks.error);
    }

    fn frequency(&self) -> f32 {
        match self.running.get_control("osc", "frequency") {
            Ok(ControlValue::Number(n)) => n,
            other => panic!("{other:?}"),
        }
    }
}

fn next_bar(beats_per_bar: Option<f32>) -> ControlWhen {
    ControlWhen::NextBar {
        beats_per_bar,
        clock: None,
    }
}

// -- no meter ----------------------------------------------------------------

/// "An invention that doesn't set it loads, plays and saves as before";
/// "With no meter set, the clock reports no bar, and its beat counts are
/// unchanged."
#[test]
fn a_clock_without_a_meter_reports_no_bar_and_keeps_its_beat_counts() {
    let rig = Rig::start(json!({}));
    rig.run_to(5.5);
    assert_eq!(rig.number("beats_per_bar"), 0.0);
    assert_eq!(rig.reading(), (0, 0, 0));
    assert!((rig.position() - 5.5).abs() < 0.05, "{}", rig.position());
    assert!(rig.number("elapsed") >= rig.position());

    let summary = rig.summary();
    assert_eq!((summary.bar, summary.beat_in_bar), (None, None));
    assert_eq!(summary.beats_per_bar, None);
    assert_eq!(summary.bpm, Some(BPM as f32));
    assert!(summary.beats.unwrap() >= 5.0);
    let wire = serde_json::to_value(&summary).unwrap();
    for key in ["bar", "beat_in_bar", "beats_per_bar"] {
        assert!(wire.get(key).is_none(), "{key} omitted: {wire}");
    }
}

/// "Its default, 0, means no meter ... loads, plays and saves as before."
#[test]
fn a_document_without_a_meter_saves_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut invention: Invention = serde_json::from_value(document(json!({}))).unwrap();
    let before = invention.to_json().unwrap();
    assert!(!before.contains("beats_per_bar"), "{before}");
    let path = dir.path().join("piece.json");
    invention.save_to_file(&path).unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(saved["modules"][0]["config"].get("beats_per_bar").is_none());
    // Saving a document with no new feature declares no newer core than it
    // did before.
    assert_eq!(saved.get("minimum_core_version"), None, "{saved}");
    invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    assert_eq!(invention.modules.len(), 3);
}

/// "The clock has a whole-number `beats_per_bar` ... config key."
#[test]
fn a_meter_in_the_config_is_kept_when_the_document_is_saved_and_read_again() {
    let invention: Invention =
        serde_json::from_value(document(json!({"beats_per_bar": 3}))).unwrap();
    let json = invention.to_json().unwrap();
    let again: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(again["modules"][0]["config"]["beats_per_bar"], json!(3));
    assert!(Invention::from_json(&json).is_ok());

    let built = ModuleRegistry::default().build("clock", RATE, &json!({ "beats_per_bar": 32 }));
    assert!(built.is_ok(), "32 beats a bar is within the range");
}

// -- counting ----------------------------------------------------------------

/// "With a meter set, the clock's telemetry reports the bar (from 1) and the
/// beat within the bar (from 1), counted from its last reset."
#[test]
fn a_meter_in_the_config_counts_bars_and_beats_from_one() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    let mut seen = Vec::new();
    for half in 0..10 {
        rig.run_to(f64::from(half) + 0.5);
        seen.push(rig.reading());
    }
    let expected: Vec<Reading> = vec![
        (1, 1, 4),
        (1, 2, 4),
        (1, 3, 4),
        (1, 4, 4),
        (2, 1, 4),
        (2, 2, 4),
        (2, 3, 4),
        (2, 4, 4),
        (3, 1, 4),
        (3, 2, 4),
    ];
    assert_eq!(seen, expected);
    assert_eq!(rig.number("beats_per_bar"), 4.0);
}

/// "The status summary carries each clock's bar, beat within the bar and
/// beats per bar beside the beat count it carries today."
#[test]
fn the_summary_reads_a_clocks_bar_beat_and_meter_beside_its_beats() {
    let rig = Rig::start(json!({ "beats_per_bar": 3 }));
    rig.run_to(4.5);
    let summary = rig.summary();
    assert_eq!(summary.id, "clock");
    assert_eq!(summary.bar, Some(2));
    assert_eq!(summary.beat_in_bar, Some(2));
    assert_eq!(summary.beats_per_bar, Some(3));
    assert_eq!(summary.bpm, Some(BPM as f32));
    assert!((summary.beats.unwrap() - 4.5).abs() < 0.1);
    let wire = serde_json::to_value(&summary).unwrap();
    assert_eq!(wire["bar"], json!(2));
    assert_eq!(wire["beat_in_bar"], json!(2));
    assert_eq!(wire["beats_per_bar"], json!(3));
}

/// The constructor reads controls by key, and a clock with no meter in force
/// reports none of the three.
#[test]
fn reading_a_summary_from_controls_leaves_bar_fields_out_without_a_meter() {
    let controls = |bar: f32, beat: f32, meter: f32| {
        move |key: &str| {
            let n = match key {
                "bpm" => 90.0,
                "elapsed" => 6.5,
                "bar" => bar,
                "beat_in_bar" => beat,
                "meter" => meter,
                _ => return None,
            };
            Some(ControlValue::Number(n))
        }
    };
    let metered = ClockSummary::read("lead", controls(2.0, 3.0, 4.0));
    assert_eq!(metered.id, "lead");
    assert_eq!(metered.bpm, Some(90.0));
    assert_eq!(metered.beats, Some(6.5));
    assert_eq!(
        (metered.bar, metered.beat_in_bar, metered.beats_per_bar),
        (Some(2), Some(3), Some(4))
    );
    let bare = ClockSummary::read("lead", controls(0.0, 0.0, 0.0));
    assert_eq!(
        (bare.bar, bare.beat_in_bar, bare.beats_per_bar),
        (None, None, None)
    );
    let missing = ClockSummary::read("lead", |_| None);
    assert_eq!(
        (missing.bpm, missing.beats, missing.bar),
        (None, None, None)
    );
    // A summary from an older daemon, without the new fields, still reads.
    let old: ClockSummary =
        serde_json::from_value(json!({ "id": "c", "bpm": 120.0, "beats": 2.0 })).unwrap();
    assert_eq!(
        (old.bar, old.beat_in_bar, old.beats_per_bar),
        (None, None, None)
    );
}

/// The three telemetry controls are read-only, like `position`.
#[test]
fn bar_beat_and_meter_cannot_be_written() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(1.5);
    for key in ["bar", "beat_in_bar", "meter"] {
        let _ = rig
            .running
            .set_control("clock", key, ControlValue::Number(9.0));
        rig.block();
        assert_ne!(rig.number(key), 9.0, "{key} is read-only telemetry");
    }
}

/// "The clock has a whole-number `beats_per_bar` control" of 0 to 32.
#[test]
fn a_meter_past_32_beats_is_refused_or_held_at_32() {
    let rig = Rig::start(json!({}));
    rig.run_to(0.5);
    let _ = rig
        .running
        .set_control("clock", "beats_per_bar", ControlValue::Number(33.0));
    rig.block();
    assert!(rig.number("beats_per_bar") <= 32.0);
    rig.set("beats_per_bar", 32.0);
    rig.run_to(2.5);
    assert_eq!(rig.reading(), (1, 3, 32));
}

// -- changing the meter ------------------------------------------------------

/// "Setting a meter on a clock that has none takes effect at once, counting
/// bars from the clock's last reset (beat 0)."
#[test]
fn a_meter_set_on_a_running_clock_without_one_counts_from_its_reset() {
    let rig = Rig::start(json!({}));
    rig.run_to(5.5);
    assert_eq!(rig.reading(), (0, 0, 0));
    rig.set("beats_per_bar", 4.0);
    rig.run_to(5.6);
    assert_eq!(rig.reading(), (2, 2, 4), "beat 5.5 is bar 2, beat 2 of 4");
    rig.run_to(8.5);
    assert_eq!(rig.reading(), (3, 1, 4));
    assert_eq!(rig.summary().bar, Some(3));
}

/// "A new meter written while the clock runs is adopted at the clock's next
/// bar line. Bars already played keep their numbers: after 2 bars of 4 and
/// then a change to 3, the bar after that is bar 3 and lasts 3 beats."
#[test]
fn a_changed_meter_is_adopted_at_the_next_bar_line_and_bars_keep_their_numbers() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(5.5);
    rig.set("beats_per_bar", 3.0);
    let mut seen = Vec::new();
    for half in 6..14 {
        rig.run_to(f64::from(half) + 0.5);
        seen.push((half, rig.summary()));
    }
    let got: Vec<_> = seen
        .iter()
        .map(|(beat, s)| {
            (
                *beat,
                s.bar.unwrap(),
                s.beat_in_bar.unwrap(),
                s.beats_per_bar.unwrap(),
            )
        })
        .collect();
    let expected = vec![
        (6, 2, 3, 4), // the rest of bar 2 is still 4 beats
        (7, 2, 4, 4),
        (8, 3, 1, 3), // bar 3 begins on the new meter
        (9, 3, 2, 3),
        (10, 3, 3, 3),
        (11, 4, 1, 3), // and lasts 3 beats
        (12, 4, 2, 3),
        (13, 4, 3, 3),
    ];
    assert_eq!(got, expected);
    assert_eq!(rig.number("beats_per_bar"), 3.0);
}

/// "A reset returns to bar 1, beat 1, and adopts any pending meter at once."
#[test]
fn a_reset_returns_to_the_first_bar_and_adopts_a_pending_meter() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(5.5);
    assert_eq!(rig.reading(), (2, 2, 4));
    rig.set("beats_per_bar", 3.0);
    rig.block();
    assert_eq!(rig.reading().2, 4, "the new meter waits for the bar line");
    rig.running
        .set_control("clock", "reset", ControlValue::Bool(true))
        .unwrap();
    for _ in 0..4 {
        rig.block();
    }
    assert_eq!(rig.reading(), (1, 1, 3));
    rig.run_to(3.5);
    assert_eq!(rig.reading(), (2, 1, 3), "bars of 3 from the reset");
}

#[test]
fn a_reset_without_a_pending_meter_starts_again_at_bar_one_beat_one() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(6.5);
    assert_eq!(rig.reading(), (2, 3, 4));
    rig.running
        .set_control("clock", "reset", ControlValue::Bool(true))
        .unwrap();
    for _ in 0..4 {
        rig.block();
    }
    assert_eq!(rig.reading(), (1, 1, 4));
    assert_eq!(rig.summary().bar, Some(1));
}

// -- timing ------------------------------------------------------------------

/// "A meter write can be timed like any other control write", and the timed
/// change is then adopted at the next bar line, not where it lands.
#[test]
fn a_timed_meter_write_lands_on_time_and_is_adopted_at_the_next_bar_line() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(4.5);
    let write = ControlWrite {
        intent: ControlWriteIntent::Perform,
        timing: ControlTiming {
            when: Some(ControlWhen::AfterBeats {
                beats: 1.0,
                clock: None,
            }),
            ttl_ms: None,
        },
        ..ControlWrite::new("clock", "beats_per_bar", ControlValue::Number(3.0))
    };
    let acks = rig.running.write_controls(&[write]);
    assert!(acks.error.is_none(), "{:?}", acks.error);
    rig.run_to(5.2);
    assert_eq!(rig.number("beats_per_bar"), 4.0, "not landed yet");
    rig.run_to(6.5);
    assert_eq!(rig.number("beats_per_bar"), 3.0, "landed at beat 5.5");
    assert_eq!(rig.reading(), (2, 3, 4), "bar 2 plays out on 4 beats");
    rig.run_to(8.5);
    assert_eq!(rig.reading(), (3, 1, 3));
    rig.run_to(11.5);
    assert_eq!(rig.reading(), (4, 1, 3));
}

/// A meter write timed on a bar line: it lands, and the clock ends up on it.
/// (Whether a write landing exactly on a bar line takes effect on that bar or
/// the next is left open by the spec, so only the settled meter is checked.)
#[test]
fn a_meter_write_timed_on_the_next_bar_ends_up_in_force() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(5.5);
    let write = ControlWrite {
        intent: ControlWriteIntent::Perform,
        timing: ControlTiming {
            when: Some(next_bar(Some(4.0))),
            ttl_ms: None,
        },
        ..ControlWrite::new("clock", "beats_per_bar", ControlValue::Number(3.0))
    };
    assert!(rig.running.write_controls(&[write]).error.is_none());
    rig.run_to(7.5);
    assert_eq!(rig.number("beats_per_bar"), 4.0);
    rig.run_to(8.5);
    assert_eq!(rig.number("beats_per_bar"), 3.0);
    rig.run_to(14.5);
    assert_eq!(rig.reading().2, 3);
}

/// "A 'next bar' request with no bar length lands on the clock's own next bar
/// line, including after a meter change."
#[test]
fn next_bar_without_a_length_lands_on_the_clocks_own_bar_line_after_a_meter_change() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(5.5);
    rig.set("beats_per_bar", 3.0);
    rig.run_to(8.5);
    assert_eq!(rig.reading(), (3, 1, 3));
    // Bar 3 runs from beat 8 to 11. A bar of 4 would end at 12.
    rig.timed_frequency(330.0, next_bar(None));
    rig.run_to(10.5);
    assert_eq!(rig.frequency(), 220.0, "not before the bar line");
    rig.run_to(11.5);
    assert_eq!(rig.frequency(), 330.0, "on the 3-beat bar's line, beat 11");
}

#[test]
fn next_bar_without_a_length_follows_a_metered_clock_from_the_start() {
    let rig = Rig::start(json!({ "beats_per_bar": 3 }));
    rig.run_to(1.5);
    rig.timed_frequency(330.0, next_bar(None));
    rig.run_to(2.5);
    assert_eq!(rig.frequency(), 220.0);
    rig.run_to(3.5);
    assert_eq!(rig.frequency(), 330.0, "the downbeat of bar 2 is beat 3");
}

/// "With no meter set, or with a bar length given, it behaves as it does
/// today": 4 beats with no meter, and multiples of a given length from beat 0
/// whatever the meter.
#[test]
fn next_bar_falls_back_to_four_beats_without_a_meter() {
    let rig = Rig::start(json!({}));
    rig.run_to(5.5);
    rig.timed_frequency(330.0, next_bar(None));
    rig.run_to(7.5);
    assert_eq!(rig.frequency(), 220.0);
    rig.run_to(8.5);
    assert_eq!(rig.frequency(), 330.0);
}

#[test]
fn next_bar_with_a_given_length_counts_multiples_from_beat_zero() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    rig.run_to(4.5);
    rig.timed_frequency(330.0, next_bar(Some(3.0)));
    rig.run_to(5.5);
    assert_eq!(rig.frequency(), 220.0);
    rig.run_to(6.5);
    assert_eq!(rig.frequency(), 330.0, "beat 6 is the next multiple of 3");
}

/// The wire shape: the bar length may be left out of the JSON.
#[test]
fn next_bar_may_leave_its_length_out_of_the_json() {
    let control: ControlWhen = serde_json::from_value(json!({ "kind": "next_bar" })).unwrap();
    assert_eq!(control, next_bar(None));
    let given: ControlWhen =
        serde_json::from_value(json!({ "kind": "next_bar", "beats_per_bar": 3 })).unwrap();
    assert_eq!(given, next_bar(Some(3.0)));
    let edit: EditWhen = serde_json::from_value(json!({ "kind": "next_bar" })).unwrap();
    assert_eq!(
        edit,
        EditWhen::NextBar {
            beats_per_bar: None,
            clock: None
        }
    );
    let edit: EditWhen =
        serde_json::from_value(json!({ "kind": "next_bar", "beats_per_bar": 6 })).unwrap();
    assert_eq!(
        edit,
        EditWhen::NextBar {
            beats_per_bar: Some(6.0),
            clock: None
        }
    );
}

// -- live playback -----------------------------------------------------------

/// "Live playback shows it: a test drives a running clock through a meter
/// change and reads the bar and beat from the summary." Two bars of 4, then
/// 3: bar 3 lasts 3 beats, read every beat through `ClockSummary::read`.
#[test]
fn a_running_clock_driven_through_a_meter_change_reports_bar_and_beat_in_its_summary() {
    let rig = Rig::start(json!({ "beats_per_bar": 4 }));
    let mut trace = Vec::new();
    for beat in 0..8 {
        rig.run_to(f64::from(beat) + 0.5);
        let s = rig.summary();
        trace.push((
            s.bar.unwrap(),
            s.beat_in_bar.unwrap(),
            s.beats_per_bar.unwrap(),
        ));
    }
    assert_eq!(trace[7], (2, 4, 4), "two bars of four are about to finish");
    rig.set("beats_per_bar", 3.0);
    let mut after = Vec::new();
    for beat in 8..14 {
        rig.run_to(f64::from(beat) + 0.5);
        let s = rig.summary();
        after.push((
            s.bar.unwrap(),
            s.beat_in_bar.unwrap(),
            s.beats_per_bar.unwrap(),
        ));
    }
    assert_eq!(
        after,
        vec![
            (3, 1, 3),
            (3, 2, 3),
            (3, 3, 3),
            (4, 1, 3),
            (4, 2, 3),
            (4, 3, 3)
        ]
    );
}
