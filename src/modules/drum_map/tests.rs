use super::*;

const RATE: u32 = 48_000;
/// 16ths at 120 BPM.
const STEP: usize = 6_000;

/// Runs `map` for `steps` clock edges `STEP` apart (the first at sample 0),
/// calling `between(map, step)` before each step, and returns each output's
/// rising samples.
fn run(
    map: &mut DrumMap,
    steps: usize,
    mut between: impl FnMut(&mut DrumMap, usize),
) -> [Vec<usize>; 4] {
    let mut rises: [Vec<usize>; 4] = Default::default();
    let mut last = [0.0f32; 4];
    for step in 0..steps {
        between(map, step);
        for block in 0..STEP / 500 {
            let block_start = step * STEP + block * 500;
            for (i, sample) in map.clock[..500].iter_mut().enumerate() {
                let into_step = block * 500 + i;
                *sample = if into_step < STEP / 4 { 1.0 } else { 0.0 };
            }
            map.process(500);
            for (output, rises) in rises.iter_mut().enumerate() {
                for i in 0..500 {
                    let value = map.output_block(output)[i];
                    if value > 0.5 && last[output] <= 0.5 {
                        rises.push(block_start + i);
                    }
                    last[output] = value;
                }
            }
        }
    }
    rises
}

fn configured(controls: &[(&str, f32)]) -> DrumMap {
    let mut map = DrumMap::new(RATE);
    for (key, value) in controls {
        map.set_control(key, *value).unwrap();
    }
    map
}

#[test]
fn density_zero_is_silent_whatever_the_position_or_chaos() {
    let mut map = configured(&[
        ("kick_density", 0.0),
        ("snare_density", 0.0),
        ("hat_density", 0.0),
        ("chaos", 1.0),
        ("resolution", 32.0),
    ]);
    let rises = run(&mut map, 64, |map, step| {
        map.set_control("x", (step % 5) as f32 / 4.0).unwrap();
        map.set_control("y", (step % 3) as f32 / 2.0).unwrap();
    });
    assert!(rises.iter().all(Vec::is_empty), "{rises:?}");
}

#[test]
fn full_density_plays_every_step_the_node_allows() {
    let mut map = configured(&[("x", 1.0), ("y", 1.0), ("hat_density", 1.0)]);
    let rises = run(&mut map, 16, |_, _| {});
    let allowed: Vec<usize> = (0..16)
        .filter(|step| map::weight(1.0, 1.0, 2, step * 2) > 0.0)
        .map(|step| step * STEP)
        .collect();
    assert_eq!(rises[2], allowed);
}

#[test]
fn the_same_position_and_seed_repeat_bit_for_bit() {
    let controls = [("x", 0.3), ("y", 0.7), ("chaos", 0.6), ("seed", 42.0)];
    let first = run(&mut configured(&controls), 32, |_, _| {});
    let again = run(&mut configured(&controls), 32, |_, _| {});
    assert_eq!(first, again);
    let other = run(
        &mut configured(&[controls[0], controls[1], controls[2], ("seed", 43.0)]),
        32,
        |_, _| {},
    );
    assert_ne!(first, other, "another seed varies the bars");
}

#[test]
fn moving_the_position_mid_bar_keeps_every_hit_on_a_clock_edge() {
    let mut map = configured(&[("hat_density", 0.9), ("chaos", 0.3)]);
    let rises = run(&mut map, 48, |map, step| {
        map.set_control("x", (step * 7 % 11) as f32 / 10.0).unwrap();
        map.set_control("y", (step * 3 % 7) as f32 / 6.0).unwrap();
    });
    assert!(rises.iter().any(|output| !output.is_empty()));
    for output in &rises {
        assert!(output.iter().all(|sample| sample % STEP == 0), "{output:?}");
    }
}

#[test]
fn swing_delays_only_the_off_beat_sixteenths() {
    let mut map = configured(&[("mode", 1.0), ("hat_density", 1.0), ("swing", 0.5)]);
    let rises = run(&mut map, 16, |_, _| {});
    let expected: Vec<usize> = (0..16)
        .map(|step| step * STEP + if step % 2 == 1 { STEP / 4 } else { 0 })
        .collect();
    assert_eq!(rises[2], expected);
}

#[test]
fn resolution_32_adds_a_step_half_way_through_each_swung_sixteenth() {
    let mut map = configured(&[
        ("mode", 1.0),
        ("hat_density", 1.0),
        ("resolution", 32.0),
        ("swing", 0.5),
    ]);
    let rises = run(&mut map, 4, |_, _| {});
    let (late, half) = (STEP / 4, (STEP + STEP / 4) / 2);
    assert_eq!(
        rises[2],
        [
            0,
            half,
            STEP + late,
            STEP + half,
            2 * STEP,
            2 * STEP + half,
            3 * STEP + late,
            3 * STEP + half
        ]
    );
}

#[test]
fn euclidean_mode_spreads_density_times_steps_hits() {
    let mut map = configured(&[("mode", 1.0), ("kick_density", 0.25)]);
    let rises = run(&mut map, 16, |_, _| {});
    assert_eq!(rises[0], [0, 4 * STEP, 8 * STEP, 12 * STEP]);
    assert_eq!(rises[3], [0], "the downbeat is accented");
}

#[test]
fn reset_makes_the_next_edge_the_downbeat() {
    let mut map = configured(&[("mode", 1.0), ("kick_density", 0.0625)]);
    let rises = run(&mut map, 8, |map, step| {
        let reset = if step == 5 { 1.0 } else { 0.0 };
        map.set_input("reset", reset).unwrap();
    });
    assert_eq!(rises[0], [0, 5 * STEP]);
}

#[test]
fn accents_only_sound_with_a_hit() {
    let mut map = configured(&[("hat_density", 0.8), ("kick_density", 0.8)]);
    let rises = run(&mut map, 32, |_, _| {});
    assert!(!rises[3].is_empty());
    for accent in &rises[3] {
        assert!(rises[..3].iter().any(|voice| voice.contains(accent)));
    }
}

#[test]
fn triggers_are_five_milliseconds_and_restart_with_a_dip() {
    let mut map = DrumMap::new(RATE);
    map.fire(1);
    map.process(10);
    assert!(map.output_block(0)[..10].iter().all(|v| *v == 1.0));
    map.fire(1);
    map.process(300);
    let kick = &map.output_block(0)[..300];
    assert_eq!(kick[0], 0.0, "a restart dips for one sample");
    assert_eq!(kick.iter().filter(|v| **v == 1.0).count(), 239);
}

#[test]
fn running_never_allocates() {
    use crate::alloc_counter::allocator_events;
    use crate::audio_thread::AudioThreadScope;
    let mut map = configured(&[("chaos", 0.5), ("resolution", 32.0), ("swing", 0.4)]);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        for block in 0..96 {
            map.clock.fill(if block % 3 == 0 { 1.0 } else { 0.0 });
            map.process(MAX_BLOCK);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn a_paused_clock_keeps_its_step_and_a_real_slowdown_is_taken() {
    let mut map = DrumMap::new(RATE);
    let edge = |map: &mut DrumMap, gap: usize| {
        map.clock[0] = 1.0;
        map.process(1);
        map.clock[0] = 0.0;
        for _ in 0..gap - 1 {
            map.process(1);
        }
    };
    edge(&mut map, 6_000);
    edge(&mut map, 60_000);
    edge(&mut map, 30_000);
    assert_eq!(
        map.period, 6_000,
        "the first edge after a pause keeps the step"
    );
    edge(&mut map, 30_000);
    assert_eq!(map.period, 30_000, "a second long gap is a slower tempo");
}

#[test]
fn the_generated_nodes_are_current() {
    let python = |args: &[&str]| {
        std::process::Command::new("python3")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .is_ok_and(|output| output.status.success())
    };
    // Without a working python3 there is nothing to compare with.
    if python(&["--version"]) {
        let current = python(&["scripts/generate_drum_map.py", "--check"]);
        assert!(current, "run python3 scripts/generate_drum_map.py");
    }
}
