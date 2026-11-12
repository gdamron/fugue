//! Pausing a running piece, on what the acceptance tests cannot see: the
//! audio thread's allocations, the meters, and the state after a stop.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::manual_backend::start_manual;

fn running_sine() -> (RunningInvention, crate::invention::manual_backend::Pump) {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
                { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#,
    )
    .unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    start_manual(runtime)
}

#[test]
fn pausing_holding_and_resuming_allocate_and_free_nothing_on_the_audio_thread() {
    let (running, pump) = running_sine();
    pump.render(4);
    running.pause();
    // The fade out, then blocks held, one of them taking a control write.
    for block in 0..12 {
        if block == 10 {
            running
                .set_control("osc", "frequency", 220.0.into())
                .unwrap();
        }
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "paused block {block}");
    }
    running.resume();
    for block in 0..10 {
        let ((), allocs, frees) = allocator_events(|| pump.block());
        assert_eq!((allocs, frees), (0, 0), "resumed block {block}");
    }
}

#[test]
fn a_held_piece_is_metered_as_nothing_playing() {
    let (running, pump) = running_sine();
    pump.render(4);
    assert!(running.master_meter().frames > 0);
    running.pause();
    pump.render(10);
    let faded = running.master_meter();
    assert_eq!(
        faded.frames, 480,
        "the meter reads the fade out, then nothing"
    );
    pump.render(10);
    let held = running.master_meter();
    assert_eq!((held.frames, held.left_peak), (0, 0.0));
    assert_eq!(held.end_frame, faded.end_frame, "the audio clock holds");
    running.resume();
    pump.render(1);
    assert_eq!(running.master_meter().frames, 64);
}

#[test]
fn a_stopped_piece_is_neither_running_nor_paused() {
    let (running, pump) = running_sine();
    running.pause();
    pump.render(1);
    let snapshot = running.snapshot();
    assert!(snapshot.status().paused);
    running.stop();
    let status = snapshot.status();
    assert!(!status.running && !status.paused && !status.playing());
}

#[test]
fn a_pause_leaves_the_request_log_inexact() {
    let (running, pump) = running_sine();
    pump.render(2);
    assert!(running.request_log().is_exact());
    running.resume();
    assert!(running.request_log().is_exact(), "resuming a playing piece");
    running.pause();
    running.pause();
    pump.render(10);
    running.resume();
    pump.render(10);
    let log = running.request_log();
    assert!(!log.is_exact(), "a replay neither fades nor holds");
    assert_eq!(log.unlogged, 2, "the pause and the resume, once each");
}

#[test]
fn a_metered_clock_holds_its_bar_and_a_next_bar_write_lands_on_its_line_after_a_resume() {
    use crate::rpc::{ControlTiming, ControlWhen};
    use crate::{ControlValue, ControlWrite, ControlWriteIntent};

    // 225 bpm at 48 kHz: a beat is 200 blocks of 64 frames; bars of 3.
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": 225.0, "beats_per_bar": 3 } },
                { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#,
    )
    .unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let number = |module: &str, key: &str| match running.get_control(module, key) {
        Ok(ControlValue::Number(n)) => n,
        other => panic!("{module}.{key}: {other:?}"),
    };
    let meter = || {
        (
            number("clock", "elapsed"),
            number("clock", "bar"),
            number("clock", "beat_in_bar"),
        )
    };
    while number("clock", "elapsed") < 4.5 {
        pump.render(1);
    }
    // Beat 4.5 is in bar 2 (beats 3 to 6); its line is at beat 6.
    let write = ControlWrite {
        intent: ControlWriteIntent::Perform,
        timing: ControlTiming {
            when: Some(ControlWhen::NextBar {
                beats_per_bar: None,
                clock: None,
            }),
            ttl_ms: None,
        },
        ..ControlWrite::new("osc", "frequency", ControlValue::Number(330.0))
    };
    assert!(running.write_controls(&[write]).error.is_none());

    running.pause();
    pump.render(10);
    let held = meter();
    assert_eq!((held.1, held.2), (2.0, 2.0), "bar 2, beat 2: {held:?}");
    pump.render(1_000);
    assert_eq!(meter(), held, "the bar and beat hold through the pause");
    assert_eq!(number("osc", "frequency"), 220.0, "the write waits");

    running.resume();
    let mut before = number("clock", "elapsed");
    while number("osc", "frequency") == 220.0 {
        before = number("clock", "elapsed");
        pump.render(1);
    }
    let landed = number("clock", "elapsed");
    assert!(
        before < 6.0 && landed >= 6.0 && landed < 6.0 + 1.0 / 200.0,
        "lands on the bar line at beat 6: {before} then {landed}"
    );
    assert_eq!(
        (number("clock", "bar"), number("clock", "beat_in_bar")),
        (3.0, 1.0)
    );
}
