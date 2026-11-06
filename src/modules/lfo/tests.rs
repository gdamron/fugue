//! Tests for the LFO.

use super::*;

#[test]
fn test_lfo_controls() {
    let mut lfo = Lfo::new(1000);

    // Test control metadata
    let controls = lfo.controls();
    assert_eq!(controls.len(), 6);
    assert_eq!(controls[0].key, "rate");
    assert_eq!(controls[1].key, "rate_mod_depth");
    assert_eq!(controls[2].key, "waveform");

    // Test get/set controls
    lfo.set_control("rate", 5.0).unwrap();
    assert_eq!(lfo.get_control("rate").unwrap(), 5.0);

    lfo.set_control("waveform", 2.0).unwrap(); // Sawtooth
    assert_eq!(lfo.get_control("waveform").unwrap(), 2.0);
}

#[test]
fn test_lfo_sine_output_range() {
    let mut lfo = Lfo::new(1000);
    lfo.set_rate(10.0);

    let mut min = f32::MAX;
    let mut max = f32::MIN;

    for _ in 0..100 {
        lfo.process(1);
        let out = lfo.get_output("bipolar").unwrap();
        let out_uni = lfo.get_output("unipolar").unwrap();

        min = min.min(out);
        max = max.max(out);

        assert!((0.0..=1.0).contains(&out_uni));
    }

    assert!(min < -0.9, "min was {}", min);
    assert!(max > 0.9, "max was {}", max);
}

#[test]
fn rate_mod_adds_hz_scaled_by_its_depth() {
    let render = |depth: f32, rate_mod: f32| {
        let mut lfo = Lfo::new(1000);
        lfo.set_rate(2.0);
        lfo.set_control("rate_mod_depth", depth).unwrap();
        lfo.set_input("rate_mod", rate_mod).unwrap();
        (0..100)
            .map(|_| {
                lfo.process(1);
                lfo.get_output("bipolar").unwrap()
            })
            .collect::<Vec<f32>>()
    };

    // Depth scales the input: 1 Hz of `rate_mod` at depth 1.0 is the
    // same added Hz as 2 units at depth 0.5.
    assert_eq!(render(1.0, 1.0), render(0.5, 2.0));
    // Depth 0 ignores the input.
    assert_eq!(render(0.0, 50.0), render(0.0, 0.0));
    assert_ne!(render(1.0, 50.0), render(0.0, 50.0));
}

#[test]
fn test_lfo_factory() {
    let factory = LfoFactory;
    assert_eq!(ModuleFactory::type_id(&factory), "lfo");

    let config = serde_json::json!({
        "rate": 5.0,
        "waveform": "triangle"
    });

    let result = factory.build(44100, &config).unwrap();

    let module = result.module.module();
    assert_eq!(module.name(), "Lfo");

    assert!(result.control_surface.is_some());
}

#[test]
fn a_waveform_change_keeps_the_phase() {
    let mut lfo = Lfo::new(1000);
    lfo.set_rate(10.0);
    lfo.process(25);
    let phase = lfo.phase;
    assert!(phase > 0.0);

    lfo.set_control("waveform", 2.0).unwrap();
    assert_eq!(lfo.phase, phase, "the phase is not reset");
    lfo.process(1);
    // A sawtooth read at the phase the sine had reached.
    let expected = 2.0 * phase - 1.0;
    assert_eq!(lfo.get_output("bipolar").unwrap(), expected);
}

#[test]
fn every_waveform_spelling_still_loads() {
    let registry = crate::ModuleRegistry::default();
    for (spelled, read) in [
        ("sine", "sine"),
        ("Square", "square"),
        ("sawtooth", "sawtooth"),
        ("SAW", "sawtooth"),
        ("saw", "sawtooth"),
        ("triangle", "triangle"),
        ("tri", "triangle"),
    ] {
        let config = serde_json::json!({ "waveform": spelled });
        let built = registry.build("lfo", 48_000, &config).unwrap();
        let surface = built.control_surface.unwrap();
        assert_eq!(surface.get_control("waveform").unwrap(), read.into());
        surface.set_control("waveform", spelled.into()).unwrap();
    }
    let config = serde_json::json!({ "waveform": "wobble" });
    assert!(registry.build("lfo", 48_000, &config).is_err());
    // Anything but text still leaves the default.
    for config in [
        serde_json::json!({ "waveform": null }),
        serde_json::json!({ "waveform": 3 }),
    ] {
        let built = registry.build("lfo", 48_000, &config).unwrap();
        let surface = built.control_surface.unwrap();
        assert_eq!(surface.get_control("waveform").unwrap(), "sine".into());
    }
}

#[test]
fn controls_clamp_as_they_always_did() {
    let mut lfo = Lfo::new(1000);
    lfo.set_control("rate", 500.0).unwrap();
    assert_eq!(lfo.get_control("rate").unwrap(), 100.0);
    lfo.set_control("rate", 0.0).unwrap();
    assert_eq!(lfo.get_control("rate").unwrap(), 0.001);
    lfo.set_control("rate_mod_depth", -3.0).unwrap();
    assert_eq!(lfo.get_control("rate_mod_depth").unwrap(), 0.0);
    assert!(lfo.set_control("rate", f32::NAN).is_err());
}

#[test]
fn a_quarter_note_at_120_bpm_is_two_hz_whatever_the_rate() {
    let mut lfo = Lfo::new(48_000);
    lfo.set_rate(7.0);
    lfo.set_control("division", 3.0).unwrap(); // quarter_note
    lfo.set_control("waveform", 2.0).unwrap(); // sawtooth: phase is plain
    lfo.process(64);
    let start = lfo.phase;
    for _ in 0..375 {
        lfo.process(64);
    }
    // 375 blocks of 64 is 24000 frames: one 0.5 s cycle, back where it was.
    assert!((lfo.phase - start).abs() < 1e-3, "{} vs {start}", lfo.phase);
}

#[test]
fn a_patched_tempo_overrides_the_control() {
    let mut lfo = Lfo::new(1000);
    lfo.set_control("division", 3.0).unwrap();
    lfo.set_control("bpm", 60.0).unwrap();
    lfo.set_input("bpm", 240.0).unwrap();
    lfo.process(1);
    assert_eq!(lfo.synced, Some(4.0));
    // Unpatched, the control's tempo times it again.
    lfo.set_input_connected(2, false);
    lfo.process(1);
    assert_eq!(lfo.synced, Some(1.0));
}

#[test]
fn a_phase_change_glides_instead_of_stepping() {
    let mut lfo = Lfo::new(48_000);
    lfo.set_rate(1.0);
    let mut last = {
        lfo.process(1);
        lfo.get_output("bipolar").unwrap()
    };
    lfo.set_control("phase", 0.5).unwrap();
    let mut largest = 0.0f32;
    for _ in 0..2_000 {
        lfo.process(1);
        let out = lfo.get_output("bipolar").unwrap();
        largest = largest.max((out - last).abs());
        last = out;
    }
    // Half a cycle over 20 ms (960 frames) moves at most 2π·0.5/960 a frame.
    assert!(largest < 4e-3, "largest step {largest}");
}

#[test]
fn a_configured_phase_starts_there() {
    let registry = crate::ModuleRegistry::default();
    let config = serde_json::json!({ "phase": 0.25, "rate": 1.0 });
    let mut built = registry.build("lfo", 48_000, &config).unwrap();
    let module = built.module.module_mut();
    module.process(1);
    assert!(module.get_output("bipolar").unwrap() > 0.999);
}

#[test]
fn a_phase_change_takes_the_short_way_round() {
    let mut lfo = Lfo::new(48_000);
    lfo.set_rate(0.001);
    lfo.set_control("phase", 0.9).unwrap();
    lfo.offset.settle();
    lfo.set_control("phase", 0.1).unwrap();
    let mut lowest = 1.0f32;
    for _ in 0..2_000 {
        lfo.process(1);
        lowest = lowest.min(lfo.get_output("bipolar").unwrap());
    }
    // Forward through 1.0 the sine stays above sin(2π·0.9); back the long
    // way it would pass 0.75 and reach −1.
    assert!(lowest > -0.6, "lowest {lowest}");
    assert!(lfo.get_output("bipolar").unwrap() > 0.58);
}
