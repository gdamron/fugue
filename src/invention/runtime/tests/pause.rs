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
