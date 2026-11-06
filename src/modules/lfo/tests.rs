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
    lfo.process(1);
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

#[test]
fn a_phase_written_before_the_first_block_starts_there() {
    let mut lfo = Lfo::new(48_000);
    lfo.set_rate(1.0);
    lfo.set_control("phase", 0.25).unwrap();
    lfo.process(1);
    assert!(lfo.get_output("bipolar").unwrap() > 0.999);
}

/// Synced, the lfo holds the beat over a long run: after five minutes at
/// 120 bpm its phase is within 1 ms of the tempo's, at the slowest division
/// (a whole note, two seconds) and a fast one.
#[test]
fn a_synced_lfo_holds_the_beat_for_five_minutes() {
    for (division, position, cycle_frames) in [
        ("whole_note", 1.0, 96_000usize),
        ("16th_triplet", 14.0, 4_000),
    ] {
        let mut lfo = Lfo::new(48_000);
        lfo.set_control("division", position).unwrap();
        let frames = 5 * 60 * 48_000;
        for _ in 0..frames / 128 {
            lfo.process(128);
        }
        let expected = (frames % cycle_frames) as f64 / cycle_frames as f64;
        let mut error = (lfo.precise - expected).abs();
        error = error.min(1.0 - error);
        let seconds = error * cycle_frames as f64 / 48_000.0;
        assert!(seconds < 1e-3, "{division}: {seconds} s off the beat");
        // The output (the last block's first sample) follows the phase too.
        let out = lfo.get_output("bipolar").unwrap();
        let first = (frames - 128) % cycle_frames;
        let want = (2.0 * std::f64::consts::PI * first as f64 / cycle_frames as f64).sin();
        assert!(
            (f64::from(out) - want).abs() < 1e-3,
            "{division}: {out} vs {want}"
        );
    }
}

/// The rate a free-running sine lfo plays at, from the spacing of the rising
/// zero crossings of its output over `seconds`, in Hz.
fn rendered_rate(sample_rate: u32, rate: f32, seconds: usize) -> f64 {
    const BLOCK: usize = 128;
    let mut lfo = Lfo::new(sample_rate);
    lfo.set_rate(rate);
    lfo.set_waveform(OscillatorType::Sine);
    let total = sample_rate as usize * seconds;
    let (mut first, mut last, mut cycles) = (None, 0.0f64, 0u32);
    let mut prev = 0.0f32;
    for start in (0..total).step_by(BLOCK) {
        lfo.process(BLOCK.min(total - start));
        for (k, &sample) in lfo.output_block(0)[..BLOCK.min(total - start)]
            .iter()
            .enumerate()
        {
            if prev < 0.0 && sample >= 0.0 {
                let at = (start + k - 1) as f64 + f64::from(-prev) / f64::from(sample - prev);
                match first {
                    None => first = Some(at),
                    Some(_) => {
                        last = at;
                        cycles += 1;
                    }
                }
            }
            prev = sample;
        }
    }
    f64::from(cycles) * f64::from(sample_rate) / (last - first.unwrap())
}

/// Free-running, the lfo keeps its rate however slow: within 0.001% of it
/// over two minutes (an `f32` phase was a percent off at 0.05 Hz).
#[test]
fn a_free_running_lfo_keeps_its_rate_when_slow() {
    for sample_rate in [44_100, 48_000] {
        for rate in [0.05f32, 0.1, 0.25, 1.0, 3.0, 20.0] {
            let measured = rendered_rate(sample_rate, rate, 120);
            let error = (measured / f64::from(rate) - 1.0).abs();
            assert!(
                error < 1e-5,
                "{rate} Hz at {sample_rate}: measured {measured}, {}% off",
                error * 100.0
            );
        }
    }
}

/// Switching between free-running and synced carries the phase across
/// without a step, both ways.
#[test]
fn switching_between_free_and_synced_carries_the_phase() {
    let mut lfo = Lfo::new(48_000);
    lfo.set_rate(1.7);
    for _ in 0..31 {
        lfo.process(128);
    }
    let free = lfo.output_block(0)[127];
    lfo.set_control("division", 3.0).unwrap(); // quarter note
    lfo.process(1);
    let synced = lfo.output_block(0)[0];
    assert!((synced - free).abs() < 0.01, "{free} then {synced}");
    lfo.set_control("division", 0.0).unwrap(); // off
    lfo.process(1);
    let free_again = lfo.output_block(0)[0];
    assert!(
        (free_again - synced).abs() < 0.01,
        "{synced} then {free_again}"
    );
}
