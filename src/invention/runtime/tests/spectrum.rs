//! The master spectrum tap, end to end through a running invention.

use super::*;
use crate::rpc::{SpectrogramEncoding, SpectrogramMagnitudes};
use crate::spectrum::{SpectrumAnalyzer, SpectrumConfig};

fn running_sine() -> RunningInvention {
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
    runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap()
}

#[test]
fn spectrum_reader_hears_the_master_output() {
    let running = running_sine();
    assert!(
        !running.spectrum_collecting(),
        "collection stays off until asked for"
    );

    let mut reader = running.spectrum_reader();
    assert!(
        running.spectrum_collecting(),
        "a reader turns collection on"
    );

    let mut out = vec![0.0; 4_096];
    let mut loudest = 0.0f32;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(25));
        let read = reader.read(&mut out);
        loudest = out[..read.count]
            .iter()
            .fold(loudest, |peak, sample| peak.max(sample.abs()));
        if loudest > 0.1 {
            break;
        }
    }
    assert!(loudest > 0.1, "the tap carried only {loudest} of a sine");

    drop(reader);
    assert!(
        !running.spectrum_collecting(),
        "a dropped reader stops collection"
    );
    running.stop();
}

#[test]
fn an_analyser_finds_the_tone_in_the_master_output() {
    let running = running_sine();
    let config = SpectrumConfig {
        fft_size: 2048,
        hop_size: 1024,
        encoding: SpectrogramEncoding::F32Json,
        ..Default::default()
    };
    let bin_hz = 48_000.0 / config.fft_size as f32;
    let mut analyzer =
        SpectrumAnalyzer::new(running.spectrum_reader(), 48_000, "master:1", config).unwrap();

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
    let SpectrogramMagnitudes::Numbers(levels) = &tile.magnitudes else {
        panic!("asked for decibels as numbers");
    };
    let peak = levels[..bin_count]
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

    drop(analyzer);
    assert!(
        !running.spectrum_collecting(),
        "a finished analyser stops collection"
    );
    running.stop();
}
