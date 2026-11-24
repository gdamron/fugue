//! The views reading neither allocates nor frees, from the first read on.

use crate::alloc_counter::allocator_events;
use crate::RenderEngine;

const PIECE: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn engine() -> RenderEngine {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(PIECE).unwrap();
    let mut out = vec![0.0; 512];
    engine.render_interleaved(&mut out).unwrap();
    engine
}

#[test]
fn the_first_read_of_levels_alone_costs_nothing() {
    let mut engine = engine();
    let (_, allocs, frees) = allocator_events(|| engine.take_levels());
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn the_first_read_of_a_clocks_beats_costs_nothing() {
    let engine = engine();
    let (beats, allocs, frees) = allocator_events(|| engine.elapsed_beats("clock"));
    assert!(beats.is_some());
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn every_read_costs_nothing() {
    let mut engine = engine();
    for clock in [Some("clock"), Some("missing"), None, Some("clock")] {
        let (reading, allocs, frees) = allocator_events(|| engine.take_view_reading(clock));
        assert!(reading.is_some());
        assert_eq!((allocs, frees), (0, 0), "{clock:?}");
    }
}
