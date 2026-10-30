//! Output taps on a running invention: their clock is the backend's, and
//! stopping closes them.

use super::*;

#[test]
fn a_tap_counts_the_backends_frames_and_closes_as_the_invention_stops() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#,
    )
    .unwrap();
    // Built at one rate and rendered at another, as the master meter's
    // test is.
    const BACKEND_RATE: u32 = 48_000;
    let (runtime, _) = InventionBuilder::new(44_100).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(BACKEND_RATE))
        .unwrap();
    let mut tap = running.tap_output("osc", &[]).unwrap();
    let mut read = tap.read(32);
    assert!(wait_until(|| {
        read = tap.read(32);
        !read.is_empty()
    }));
    assert_eq!(read.sample_rate, BACKEND_RATE);
    assert_eq!(read.ports, ["audio"]);

    running.stop();
    let read = tap.read(32);
    assert!(!read.open && read.is_empty(), "{read:?}");
}
