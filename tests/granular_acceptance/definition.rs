//! The testable-from-outside parts of the definition of done: live
//! control writes, config handling, determinism, bounded output under
//! extreme settings, freeze holding the buffer, and the live path through
//! `RenderEngine`.

use super::rig::*;
use super::*;

/// Every control may be given in config as its starting value, and is
/// readable and writable in a running invention.
#[test]
fn every_control_starts_from_config_and_can_be_written_live() {
    let config = json!({ "mode": "stretch", "position": 0.25, "size": 0.75, "pitch": 7.0,
        "density": 0.3, "texture": 0.8, "freeze": true, "spread": 0.4, "feedback": 0.2,
        "sync": true });
    let engine = engine(live_doc(config));
    let get = |key: &str| (engine.get_control("g", key)).unwrap_or_else(|e| panic!("{key}: {e}"));
    for (key, want) in [
        ("position", 0.25f32),
        ("size", 0.75),
        ("pitch", 7.0),
        ("density", 0.3),
        ("texture", 0.8),
        ("spread", 0.4),
        ("feedback", 0.2),
    ] {
        let ControlValue::Number(got) = get(key) else {
            panic!("{key} is a number")
        };
        assert!((got - want).abs() < 1e-6, "{key} = {got}");
    }
    assert_eq!(get("mode"), ControlValue::from("stretch"));
    assert_eq!((get("freeze"), get("sync")), (true.into(), true.into()));
    let write = |key: &str, v: ControlValue| (engine.set_control("g", key, v)).expect(key);
    write("mode", "looping_delay".into());
    write("freeze", false.into());
    write("pitch", (-5.0f32).into());
    assert_eq!(get("mode"), ControlValue::from("looping_delay"));
    assert_eq!(get("freeze"), ControlValue::Bool(false));
    assert_eq!(get("pitch"), ControlValue::Number(-5.0));
    assert!(engine
        .set_control("g", "mode", ControlValue::from("no_such_mode"))
        .is_err());
}

#[test]
fn buffer_length_zero_or_below_is_refused_and_huge_values_build() {
    for bad in [0.0, -1.0, -0.001] {
        assert!(
            try_granular(json!({ "buffer_length": bad })).is_err(),
            "buffer_length {bad} is refused"
        );
    }
    for ok in [0.01, 4.0, 60.0, 61.0, 1.0e9] {
        assert!(
            try_granular(json!({ "buffer_length": ok })).is_ok(),
            "buffer_length {ok} builds (clamps to 60)"
        );
    }
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

fn random_grains(seed: u64, mode: f32) -> (Vec<f32>, Vec<f32>) {
    let start = [
        ("mode", mode),
        ("density", 0.8),
        ("spread", 1.0),
        ("size", 0.2),
    ];
    let feed = Feed::mono(noise(s(3.0), 0.8, 17));
    render_granular(
        json!({ "buffer_length": 1.0, "seed": seed }),
        &start,
        &[],
        &feed,
    )
}

/// "Same seed, same input, same controls: bit-identical output."
#[test]
fn same_seed_gives_bit_identical_output_in_every_mode() {
    for (name, mode) in MODES {
        let (a, b) = (random_grains(42, mode), random_grains(42, mode));
        assert!(peak(&a.0) > 0.05, "{name}: output is silent");
        assert_eq!((bits(&a.0), bits(&a.1)), (bits(&b.0), bits(&b.1)), "{name}");
    }
}

/// Assumption: the seed drives the grain randomness, so another seed gives
/// other grains (random stereo placement at `spread` 1).
#[test]
fn another_seed_gives_other_grains() {
    let (a, b) = (random_grains(1, GRANULAR), random_grains(2, GRANULAR));
    assert_ne!(bits(&a.0), bits(&b.0));
}

/// The same invention rendered twice through the engine is bit-identical.
#[test]
fn same_invention_renders_bit_identically_twice() {
    let doc = live_doc(json!({ "buffer_length": 1.0, "seed": 4, "density": 0.9, "spread": 1.0 }));
    let (a, b) = (
        render(&mut engine(doc.clone()), s(2.0)),
        render(&mut engine(doc), s(2.0)),
    );
    assert!(peak(&a.0) > 0.05, "output is silent");
    assert_eq!((bits(&a.0), bits(&a.1)), (bits(&b.0), bits(&b.1)));
}

/// Output stays finite and within +/-1 under every extreme of pitch, size,
/// density and position, with full-scale noise in, full feedback and spread.
#[test]
fn output_is_bounded_under_extreme_settings() {
    let feed = Feed::mono(noise(s(1.0), 1.0, 3)).with_trigger(gates(
        s(1.0),
        (0..8).map(|k| k * 6_000),
        48,
    ));
    for (name, mode) in MODES {
        for pitch in [-24.0, 24.0] {
            for size in [0.0, 1.0] {
                for density in [0.0, 1.0] {
                    for position in [0.0, 1.0] {
                        let start = [
                            ("mode", mode),
                            ("pitch", pitch),
                            ("size", size),
                            ("density", density),
                            ("position", position),
                            ("feedback", 1.0),
                            ("spread", 1.0),
                        ];
                        let config = json!({ "buffer_length": 0.5, "seed": 2 });
                        let (l, r) = render_granular(config, &start, &[], &feed);
                        for ch in [l, r] {
                            let bad = ch.iter().find(|v| !v.is_finite() || v.abs() > 1.0);
                            assert!(
                                bad.is_none(),
                                "{name} pitch {pitch} size {size} density {density} position {position}: {bad:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Freeze holds the buffer: two runs hear the same input until just after
/// the freeze, then different input (noise against the sine). Playback from
/// 250 ms after the freeze is bit-identical.
/// Assumption: recording stops fading within 100 ms of the freeze.
#[test]
fn freeze_holds_the_buffer_whatever_the_input_does() {
    let (frozen_at, diverge) = (s(2.0), s(2.0) + s(0.1));
    let tone = sine(s(4.0), 220.0, 0.5);
    let mut changed = tone.clone();
    for (v, n) in changed[diverge..].iter_mut().zip(noise(s(2.0), 0.8, 9)) {
        *v = n;
    }
    for (name, mode) in MODES {
        let run = |input: &Vec<f32>| {
            let config = json!({ "buffer_length": 1.0, "seed": 6 });
            let events = [(frozen_at, "freeze", 1.0)];
            let start = [("mode", mode), ("density", 0.8), ("spread", 0.5)];
            render_granular(config, &start, &events, &Feed::mono(input.clone()))
        };
        let (a, b) = (run(&tone), run(&changed));
        let held = frozen_at + s(0.25)..s(4.0);
        assert!(
            rms(&a.0[held.clone()]) > 0.02,
            "{name}: the frozen buffer is silent"
        );
        let worst = (a.0[held.clone()].iter().zip(&b.0[held]))
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(
            worst <= 1e-6,
            "{name}: the input leaked into the frozen buffer ({worst})"
        );
    }
}

/// Live playback: the module in an invention renders sound, stays inside
/// +/-1 across live mode and freeze changes, and keeps the control values.
#[test]
fn live_invention_plays_and_follows_live_control_changes() {
    let mut engine = engine(live_doc(json!({ "buffer_length": 2.0, "seed": 1 })));
    let (l, r) = render(&mut engine, s(3.0));
    assert!(
        rms(&l[s(1.5)..]) > 0.01 && rms(&r[s(1.5)..]) > 0.01,
        "silent live output"
    );
    for (key, value) in [
        ("mode", "stretch"),
        ("mode", "looping_delay"),
        ("mode", "granular"),
    ] {
        (engine.set_control("g", key, value.into())).expect(key);
        let (l, r) = render(&mut engine, s(0.5));
        assert!(peak(&l).max(peak(&r)) <= 1.0, "{key} = {value}");
    }
    (engine.set_control("g", "freeze", true.into())).expect("freeze");
    let (l, r) = render(&mut engine, s(1.0));
    assert!(peak(&l).max(peak(&r)) <= 1.0);
    assert!(rms(&l[s(0.5)..]) > 0.01, "frozen live output is silent");
}
