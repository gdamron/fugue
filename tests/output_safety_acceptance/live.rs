//! Live playback: a scripted source into a dac, driven like a device callback.

use super::probe::Live;
use super::*;
use fugue::MeterReading;
use std::sync::atomic::Ordering;

fn reference(x: f32) -> f32 {
    let mut dac = DacModule::new();
    let i = dac.input_port_index("audio_left").unwrap();
    dac.input_block_mut(i)[0] = x;
    dac.process(1);
    dac.sink_block().0[0]
}

/// Left: four non-finite of eight; right: two of eight; nothing on `audio`.
fn mixed() -> Value {
    json!({
        "audio_left": [0.25, "nan", 3.0, "inf", -0.5, "-inf", 0.125, "nan"],
        "audio_right": [0.5, 0.5, "nan", 0.75, "inf", 0.25, -3.0, 0.5],
    })
}

#[test]
fn nonfinite_samples_reach_the_device_as_silence_and_finite_ones_are_unchanged() {
    let live = Live::start(mixed());
    let (l, r) = live.render(256);
    let left = [
        Some(0.25),
        None,
        Some(3.0),
        None,
        Some(-0.5),
        None,
        Some(0.125),
        None,
    ];
    let right = [
        Some(0.5),
        Some(0.5),
        None,
        Some(0.75),
        None,
        Some(0.25),
        Some(-3.0),
        Some(0.5),
    ];
    for i in 0..256 {
        for (got, want, side) in [(l[i], left[i % 8], "left"), (r[i], right[i % 8], "right")] {
            match want {
                None => assert_eq!(got, 0.0, "{side}[{i}] is silence"),
                Some(x) => assert_eq!(got.to_bits(), reference(x).to_bits(), "{side}[{i}]"),
            }
        }
    }
    live.running.stop();
}

#[test]
fn meters_report_the_running_total_and_serialize_it_only_when_nonzero() {
    let live = Live::start(mixed());
    live.render(256);
    let levels = live.running.master_meter();
    assert_eq!(
        levels.non_finite_samples,
        32 * 6,
        "(4 + 2) per 8 frames, 256 frames"
    );
    assert!(levels.left_peak.is_finite() && levels.left_peak <= 1.0);
    let reading = MeterReading::new("master", &levels);
    assert_eq!(reading.non_finite_samples, 192);
    assert_eq!(
        serde_json::to_value(&reading).unwrap()["non_finite_samples"],
        192
    );

    // A total, not a window: draining again keeps it, more audio adds to it.
    assert_eq!(live.running.master_meter().non_finite_samples, 192);
    live.render(64);
    assert_eq!(live.running.master_meter().non_finite_samples, 192 + 48);
    live.running.stop();

    let clean = Live::start(json!({ "audio_left": [0.1, 0.2], "audio": [0.0] }));
    clean.render(128);
    let levels = clean.running.master_meter();
    assert_eq!(levels.non_finite_samples, 0);
    let value = serde_json::to_value(MeterReading::new("master", &levels)).unwrap();
    assert!(
        value.get("non_finite_samples").is_none(),
        "omitted at zero: {value}"
    );
    clean.running.stop();
}

#[test]
fn mono_nonfinite_counts_for_both_channels_live() {
    let live = Live::start(json!({ "audio": ["nan", 0.25] }));
    let (l, r) = live.render(64);
    for i in 0..64 {
        let want = if i % 2 == 0 { 0.0 } else { 0.25 };
        assert_eq!((l[i], r[i]), (want, want), "frame {i}");
    }
    assert_eq!(live.running.master_meter().non_finite_samples, 64);
    live.running.stop();
}

#[test]
fn flush_to_zero_is_set_in_every_callback_and_the_thread_is_left_as_found() {
    assert!(!fugue::flush_to_zero_enabled(), "off outside a render");
    let before = std::hint::black_box(subnormal()) * std::hint::black_box(0.5f32);
    assert_ne!(before, 0.0, "this thread computes subnormals by default");

    let live = Live::start(json!({}));
    for frames in [64, 17, 1, 64] {
        live.callback(frames);
        assert!(!fugue::flush_to_zero_enabled(), "restored after a callback");
    }
    assert_eq!(live.log.calls.load(Ordering::SeqCst), 4);
    let missing = live.log.flag_missing.load(Ordering::SeqCst);
    let decayed = live.log.decay_left_nonzero.load(Ordering::SeqCst);
    if FTZ_TARGET {
        assert!(!missing, "flag on inside process() of every callback");
        assert!(!decayed, "a subnormal decays to zero inside the callback");
    } else {
        assert!(missing, "other targets skip the flag");
    }
    live.running.stop();
}

#[test]
fn a_subnormal_source_sample_reaches_the_device_as_zero() {
    let live = Live::start(json!({ "audio_left": [subnormal(), 0.25] }));
    let (l, _) = live.render(64);
    if FTZ_TARGET {
        assert!(
            l.iter().step_by(2).all(|&x| x == 0.0),
            "subnormal flushed: {:?}",
            &l[..4]
        );
    }
    assert!(
        l.iter().skip(1).step_by(2).all(|&x| x == 0.25),
        "normal samples unchanged"
    );
    live.running.stop();
}
