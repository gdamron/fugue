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
    for clock in [
        Some("clock"),
        Some("missing"),
        Some("osc"),
        None,
        Some("clock"),
    ] {
        let (reading, allocs, frees) = allocator_events(|| engine.take_view_reading(clock));
        assert!(reading.is_some());
        assert_eq!((allocs, frees), (0, 0), "{clock:?}");
    }
}

#[test]
fn a_module_that_counts_no_beats_has_none() {
    let mut engine = engine();
    let reading = engine.take_view_reading(Some("osc")).unwrap();
    assert_eq!(reading.beats, None);
    assert!(engine
        .take_view_reading(Some("clock"))
        .unwrap()
        .beats
        .is_some());
}

const MIXED: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "waveform": "saw" } },
        { "id": "mix", "type": "mixer", "config": { "channel_count": 2, "levels": [0.5, 0.25] } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "mix", "to_port": "audio.0" },
        { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
        { "from": "mix", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

#[test]
fn a_number_read_agrees_with_get_control() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(MIXED).unwrap();
    let surfaces = engine.control_surfaces.lock().unwrap();
    let read = |id: &str, key: &str| {
        let surface = surfaces.get(id).unwrap();
        let shown = surface
            .get_control(key)
            .ok()
            .and_then(|v| v.as_number().ok());
        let (number, allocs, frees) = allocator_events(|| surface.read_number(key));
        assert_eq!((allocs, frees), (0, 0), "{id} {key}");
        assert_eq!(number, shown, "{id} {key}");
        number
    };
    assert_eq!(read("mix", "level.0"), Some(0.5));
    assert_eq!(read("mix", "level.1"), Some(0.25));
    // Past the mixer's two channels, a level is never listed or heard.
    assert_eq!(read("mix", "level.2"), None);
    assert_eq!(read("mix", "level.99"), None);
    assert_eq!(read("mix", "channel_count"), Some(2.0));
    // A choice names its option; it holds no number.
    assert_eq!(read("osc", "waveform"), None);
    assert!(read("osc", "frequency").is_some());
}
