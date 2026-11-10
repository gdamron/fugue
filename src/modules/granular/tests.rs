use self::controls::MODES;
use super::*;
use crate::ModuleRegistry;

const SR: u32 = 48_000;

fn build(config: serde_json::Value) -> Box<dyn Module> {
    match ModuleRegistry::default()
        .build(TYPE_ID, SR, &config)
        .unwrap()
        .module
    {
        GraphModule::Module(module) => module,
        _ => panic!("granular is a plain module"),
    }
}

/// Renders `frames` of `input(n)` into the left input, `gates(n)` into
/// trigger and clock as patched, with `controls` set first.
fn render(
    module: &mut dyn Module,
    frames: usize,
    input: impl Fn(usize) -> f32,
    gate: Option<(usize, &dyn Fn(usize) -> f32)>,
) -> (Vec<f32>, Vec<f32>) {
    module.set_input_connected(0, true);
    if let Some((port, _)) = gate {
        module.set_input_connected(port, true);
    }
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let mut at = 0;
    while at < frames {
        let n = MAX_BLOCK.min(frames - at);
        for i in 0..n {
            module.input_block_mut(0)[i] = input(at + i);
            if let Some((port, gate)) = gate {
                module.input_block_mut(port)[i] = gate(at + i);
            }
        }
        module.process(n);
        left.extend_from_slice(&module.output_block(0)[..n]);
        right.extend_from_slice(&module.output_block(1)[..n]);
        at += n;
    }
    (left, right)
}

fn sine(n: usize, hz: f32) -> f32 {
    (std::f32::consts::TAU * hz * n as f32 / SR as f32).sin()
}

fn largest_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn full_feedback_on_a_full_scale_input_stays_within_unity() {
    let mut module = Granular::with_cells(
        SR,
        DEFAULT_BUFFER_LENGTH,
        0,
        Arc::new(ControlCells::new(controls::defaults())),
    );
    for (key, value) in [("feedback", 1.0), ("density", 0.95), ("texture", 0.9)] {
        module.set_control(key, value).unwrap();
    }
    let (left, right) = render(&mut module, SR as usize * 60, |n| sine(n, 220.0), None);
    for x in left.iter().chain(&right) {
        assert!(x.is_finite() && x.abs() <= 1.0);
    }
    assert!(left.iter().any(|x| x.abs() > 0.1), "the grains are heard");
    // The output is clamped, so look inside the loop: what it records
    // stays within the soft limit's reach (1, give or take a rounding).
    let reach = 1.0 + 4.0 * f32::EPSILON;
    let recorded = (0..module.recording.len()).map(|i| module.recording.read(i, 0.0));
    for (l, r) in recorded {
        assert!(l.abs() <= reach && r.abs() <= reach, "recorded {l}, {r}");
    }
}

#[test]
fn a_trigger_starts_one_grain_on_its_frame() {
    // Density in its silent middle: only the trigger starts grains.
    let mut module =
        build(serde_json::json!({ "density": 0.5, "texture": 0.0, "size": 0.0, "position": 0.0 }));
    let edge = 30_000;
    let trigger = |n: usize| {
        if (edge..edge + 100).contains(&n) {
            1.0
        } else {
            0.0
        }
    };
    let (left, _) = render(
        module.as_mut(),
        40_000,
        |_| 0.5,
        Some((TRIGGER_IN, &trigger)),
    );
    assert!(left[..=edge].iter().all(|x| *x == 0.0));
    assert!(left[edge + 1] > 0.0);
    // One grain of about 32 ms, then silence again.
    let length = left.iter().rposition(|x| *x != 0.0).unwrap() - edge;
    assert!((1500..1600).contains(&length), "a grain of {length} frames");
}

#[test]
fn synced_grains_fall_on_the_clock_divisions() {
    // Grains at regular intervals, four to a pulse.
    let mut module = build(serde_json::json!({
        "sync": true, "density": 0.2, "texture": 0.0, "size": 0.0, "position": 0.0
    }));
    // The first edge comes once there is audio to read.
    let (first, period) = (2_000, 24_000);
    let clock = |n: usize| {
        if n >= first && (n - first) % period < 100 {
            1.0
        } else {
            0.0
        }
    };
    let (left, _) = render(
        module.as_mut(),
        first + period * 3,
        |_| 0.5,
        Some((CLOCK_IN, &clock)),
    );
    let onsets: Vec<usize> = (1..left.len())
        .filter(|&n| left[n] > 0.0 && left[n - 1] == 0.0)
        .map(|n| n - 1)
        .collect();
    let expected: Vec<usize> = std::iter::once(first)
        .chain(
            (1..3).flat_map(|pulse| (0..4).map(move |k| first + pulse * period + k * period / 4)),
        )
        .collect();
    assert_eq!(onsets, expected);
}

#[test]
fn freeze_keeps_playing_what_was_recorded() {
    let config = serde_json::json!({ "density": 0.9, "position": 0.2, "seed": 3 });
    let freeze_at = SR as usize;
    let render_with = |input: fn(usize) -> f32| {
        let mut module = build(config.clone());
        let (head, _) = render(module.as_mut(), freeze_at, |n| sine(n, 330.0), None);
        module.set_control("freeze", 1.0).unwrap();
        let (tail, _) = render(module.as_mut(), SR as usize, input, None);
        (head, tail)
    };
    let (_, silent) = render_with(|_| 0.0);
    let (_, loud) = render_with(|n| sine(n, 1234.0));
    assert_eq!(silent, loud, "nothing recorded after the freeze is heard");
    assert!(
        silent.iter().any(|x| x.abs() > 0.1),
        "the frozen buffer plays"
    );
}

#[test]
fn freezing_and_unfreezing_do_not_click() {
    // Regular grains with a smooth window, over a low sine.
    let config = serde_json::json!({ "density": 0.1, "texture": 0.75, "position": 0.0 });
    let mut module = build(config);
    let input = |n: usize| 0.5 * sine(n, 110.0);
    let (steady, _) = render(module.as_mut(), SR as usize * 2, input, None);
    let steady = largest_step(&steady[SR as usize..]);
    module.set_control("freeze", 1.0).unwrap();
    let (frozen, _) = render(module.as_mut(), SR as usize, input, None);
    module.set_control("freeze", 0.0).unwrap();
    let (thawed, _) = render(module.as_mut(), SR as usize, input, None);
    assert!(largest_step(&frozen) <= 1.5 * steady);
    assert!(largest_step(&thawed) <= 1.5 * steady);
}

#[test]
fn unfreezing_below_unity_pitch_does_not_click() {
    // Grains read an octave down cross the still write head while frozen;
    // once it moves again it must not overrun them.
    let config = serde_json::json!({
        "buffer_length": 1.0, "density": 0.8, "texture": 0.75, "position": 0.0,
        "pitch": -12.0, "size": 1.0, "seed": 2
    });
    let mut module = build(config);
    let input = |n: usize| 0.5 * sine(n, 110.0);
    let (steady, _) = render(module.as_mut(), SR as usize * 3, input, None);
    let steady = largest_step(&steady[SR as usize..]);
    module.set_control("freeze", 1.0).unwrap();
    let (frozen, _) = render(module.as_mut(), SR as usize, input, None);
    module.set_control("freeze", 0.0).unwrap();
    let (thawed, _) = render(module.as_mut(), SR as usize, input, None);
    for (label, part) in [("frozen", frozen), ("thawed", thawed)] {
        let step = largest_step(&part);
        assert!(step <= 1.5 * steady, "{label}: {step} vs {steady}");
    }
}

#[test]
fn grains_reading_one_delay_are_scaled_by_power_alone() {
    // As in the original, overlapping grains are scaled by one over the
    // root of how many play, which assumes they add up in power. Unpitched
    // grains at one position all read the same audio and add up in
    // amplitude, so a dense cloud comes out louder than its input: that grit
    // is kept, not corrected away. The input is quiet enough that the output
    // stays below the soft clip, so the gain is the cloud's own.
    let config = serde_json::json!({ "density": 1.0, "texture": 0.6, "position": 0.1 });
    let mut module = build(config);
    let level = 0.05;
    let (left, _) = render(
        module.as_mut(),
        SR as usize * 3,
        |n| level * sine(n, 220.0),
        None,
    );
    let tail = &left[SR as usize * 2..];
    let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
    let gain = rms / (level / 2f32.sqrt());
    // Corrected for likeness the gain would be about one; scaled by power
    // alone it is several times that.
    assert!((3.0..8.0).contains(&gain), "a gain of {gain}");
    let peak = tail.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak < 0.75, "below the soft clip, at {peak}");
}

#[test]
fn the_buffer_length_is_refused_at_zero_and_held_to_a_minute() {
    let registry = ModuleRegistry::default();
    assert!(registry
        .build(TYPE_ID, SR, &serde_json::json!({ "buffer_length": 0.0 }))
        .is_err());
    assert!(registry
        .build(
            TYPE_ID,
            SR,
            &serde_json::json!({ "buffer_length": f32::MAX })
        )
        .is_ok());
}

#[test]
fn density_reach_picks_the_clock_divisions() {
    assert_eq!(divisions(0.0), 1);
    assert_eq!(divisions(0.5), 4);
    assert_eq!(divisions(1.0), 8);
}

#[test]
fn changing_mode_crossfades_without_a_click() {
    let config = serde_json::json!({
        "density": 0.9, "texture": 0.5, "position": 0.1, "seed": 4
    });
    let mut module = build(config);
    let input = |n: usize| 0.5 * sine(n, 110.0);
    let (steady, _) = render(module.as_mut(), SR as usize * 2, input, None);
    let steady = largest_step(&steady[SR as usize..]);
    for mode in ["stretch", "granular", "stretch"] {
        let index = MODES.iter().position(|m| *m == mode).unwrap() as f32;
        module.set_control("mode", index).unwrap();
        let (after, _) = render(module.as_mut(), SR as usize / 2, input, None);
        let step = largest_step(&after);
        assert!(step <= 1.5 * steady, "to {mode}: {step} vs {steady}");
        assert!(
            after[SR as usize / 4..].iter().any(|x| x.abs() > 0.05),
            "{mode} plays"
        );
    }
}

/// A looping delay over a melody-like input, held at `position` 0.3 for
/// a second with `glide` at `held`, then swept to 1 with it off.
fn swept_delay(held: Option<f32>) -> Vec<f32> {
    let mut module = build(
        serde_json::json!({ "mode": "looping_delay", "buffer_length": 1.0, "position": 0.3 }),
    );
    if let Some(seconds) = held {
        module.set_control("glide", seconds).unwrap();
    }
    module.set_input_connected(0, true);
    let frames = 3 * SR as usize;
    let mut out = Vec::with_capacity(frames);
    let mut at = 0;
    while at < frames {
        if at >= SR as usize {
            module.set_control("glide", 0.0).unwrap();
            module
                .set_control("position", 0.3 + 0.7 * at as f32 / frames as f32)
                .unwrap();
        }
        for i in 0..MAX_BLOCK {
            let n = at + i;
            module.input_block_mut(0)[i] = sine(n, 220.0 * (1 + n / 6_000 % 3) as f32);
        }
        module.process(MAX_BLOCK);
        out.extend_from_slice(&module.output_block(0)[..MAX_BLOCK]);
        at += MAX_BLOCK;
    }
    out
}

#[test]
fn a_glide_unused_or_turned_off_leaves_the_looping_delay_as_it_was() {
    // Held still, a glide moves nothing; off, every move jumps as before.
    let untouched = swept_delay(None);
    assert!(untouched.iter().any(|x| x.abs() > 0.3), "it plays");
    assert_eq!(swept_delay(Some(0.5)), untouched);
}
