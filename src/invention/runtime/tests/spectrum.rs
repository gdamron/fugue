//! The master spectrum tap, end to end through a running invention.

use super::*;
use crate::spectrum::{SpectrumAnalyzer, SpectrumConfig};

#[test]
fn spectrum_tap_feeds_an_analyser_with_the_master_output() {
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

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    let tap = running.spectrum_tap();
    assert!(!tap.is_enabled(), "collection stays off until asked for");
    let reader = tap
        .take_reader()
        .expect("the tap hands out its reader once");

    let config = SpectrumConfig {
        fft_size: 2048,
        hop_size: 1024,
        ..Default::default()
    };
    let bin_hz = 48_000.0 / config.fft_size as f32;
    let mut analyzer = SpectrumAnalyzer::new(reader, 48_000, "master:1", config).unwrap();
    assert!(tap.is_enabled(), "an analyser turns collection on");

    // Let the audio worker render enough blocks to fill several frames.
    let mut tile = None;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(25));
        while let Some(next) = analyzer.poll() {
            tile = Some(next);
        }
        if tile.is_some() {
            break;
        }
    }
    let tile = tile.expect("the analyser should have produced a tile");
    assert!(tile.matches(analyzer.meta()));

    let bin_count = analyzer.meta().frequency.bin_count as usize;
    let frame = &tile.magnitudes_db[..bin_count];
    let peak = frame
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(bin, _)| bin)
        .unwrap();
    let peak_hz = peak as f32 * bin_hz;
    assert!(
        (peak_hz - 440.0).abs() < bin_hz,
        "expected the peak near 440 Hz, got {peak_hz} Hz"
    );

    analyzer.stop();
    assert!(!tap.is_enabled());
    drop(analyzer);
    assert!(
        tap.take_reader().is_some(),
        "a finished analyser returns the reading end"
    );
    running.stop();
}
