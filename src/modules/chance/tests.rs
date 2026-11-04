use super::*;
use crate::music::scales::{in_key, scale_degrees, strongest_degrees};
use crate::music::Note;

fn chance(config: serde_json::Value) -> Box<dyn Module> {
    match ChanceFactory.build(48_000, &config).unwrap().module {
        GraphModule::Module(module) => module,
        _ => unreachable!(),
    }
}

/// One step per 4-frame block: the clock high for two frames. Returns
/// every output at the step's first frame.
fn step(module: &mut dyn Module) -> [f32; 9] {
    module.input_block_mut(0)[..4].copy_from_slice(&[1.0, 1.0, 0.0, 0.0]);
    module.process(4);
    std::array::from_fn(|port| module.output_block(port)[0])
}

fn steps(module: &mut dyn Module, count: usize) -> Vec<[f32; 9]> {
    (0..count).map(|_| step(module)).collect()
}

#[test]
fn gate_rate_follows_bias() {
    for bias in [0.1, 0.5, 0.9] {
        let mut module = chance(serde_json::json!({ "seed": 3, "bias": bias }));
        let played = steps(module.as_mut(), 10_000);
        let rate = played.iter().filter(|out| out[0] == 1.0).count() as f32 / 10_000.0;
        assert!((rate - bias).abs() < 0.02, "bias {bias}: {rate}");
        assert!(played
            .iter()
            .all(|out| out[0] + out[1] == 1.0 && out[2] == 1.0));
    }
}

#[test]
fn gates_follow_the_clock_pulse() {
    let mut module = chance(serde_json::json!({ "seed": 1, "bias": 1.0 }));
    step(module.as_mut());
    assert_eq!(&module.output_block(0)[..4], &[1.0, 1.0, 0.0, 0.0]);
    assert_eq!(&module.output_block(2)[..4], &[1.0, 1.0, 0.0, 0.0]);
}

#[test]
fn a_locked_loop_repeats_with_its_length() {
    for length in [1, 3, 8, 16] {
        let mut module = chance(serde_json::json!({
            "seed": 9, "repetition": 1.0, "loop_length": length, "steps": 0.4,
        }));
        let played = steps(module.as_mut(), 64);
        // Channels 1 and 2 hold where they started until their gate first
        // fires, so compare from the second pass.
        for (index, out) in played.iter().enumerate().skip(2 * length) {
            assert_eq!(
                out,
                &played[index - length],
                "length {length}, step {index}"
            );
        }
    }
}

#[test]
fn a_mutating_loop_keeps_most_of_its_steps() {
    let mut module = chance(serde_json::json!({
        "seed": 4, "repetition": 0.5, "loop_length": 8, "steps": 0.4,
    }));
    let played = steps(module.as_mut(), 800);
    let kept = (8..800)
        .filter(|&i| played[i][8] == played[i - 8][8])
        .count() as f32
        / 792.0;
    // Three quarters of the steps replay at the middle of the range.
    assert!((kept - 0.75).abs() < 0.06, "{kept}");
}

#[test]
fn quantized_pitches_stay_in_key() {
    let mut module = chance(serde_json::json!({
        "seed": 5, "key": "Eb", "scale": "dorian", "spread": 1.0,
    }));
    let allowed = in_key(strongest_degrees(scale_degrees(4), 12), 3);
    for out in steps(module.as_mut(), 2_000) {
        let note = Note::from_frequency(out[5]).midi_note;
        assert!(allowed & 1 << (note % 12) != 0, "{note}");
        assert!((48..=72).contains(&note), "{note}");
    }
}

#[test]
fn the_same_seed_plays_the_same() {
    let config = serde_json::json!({ "seed": 11, "mode": "divider", "repetition": 0.3 });
    let (mut one, mut two) = (chance(config.clone()), chance(config));
    let (a, b) = (steps(one.as_mut(), 500), steps(two.as_mut(), 500));
    assert!(a
        .iter()
        .zip(&b)
        .all(|(x, y)| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits())));
    // Writing the seed starts the stream again.
    let mut one = chance(serde_json::json!({ "seed": 11 }));
    steps(one.as_mut(), 37);
    one.set_control("seed", 11.0).unwrap();
    let mut fresh = chance(serde_json::json!({ "seed": 11 }));
    let restarted: Vec<f32> = steps(one.as_mut(), 50).iter().map(|o| o[8]).collect();
    let first: Vec<f32> = steps(fresh.as_mut(), 50).iter().map(|o| o[8]).collect();
    assert_eq!(restarted, first);
}

#[test]
fn reset_starts_the_bar_again() {
    let mut module = chance(serde_json::json!({ "seed": 2, "mode": "divider", "bias": 0.0 }));
    let first = steps(module.as_mut(), 5);
    module.input_block_mut(1)[..2].copy_from_slice(&[1.0, 0.0]);
    module.input_block_mut(0)[..2].fill(0.0);
    module.process(2);
    module.input_block_mut(1)[..4].fill(0.0);
    let again = steps(module.as_mut(), 5);
    let gates = |played: &[[f32; 9]]| played.iter().map(|o| (o[0], o[1])).collect::<Vec<_>>();
    assert_eq!(gates(&first), gates(&again));
    assert_eq!(gates(&first)[0], (1.0, 0.0));
    assert_eq!(gates(&first)[4], (0.0, 1.0));
}

#[test]
fn controls_are_refused_out_of_their_options() {
    let mut module = chance(serde_json::json!({}));
    assert!(module.set_control("key", 12.0).is_err());
    assert!(module.set_control("scale", 99.0).is_err());
    assert!(module.set_control("mode", 3.0).is_err());
    module.set_control("loop_length", 40.0).unwrap();
    assert_eq!(module.get_control("loop_length").unwrap(), 16.0);
}

/// Every frame of `gate_1`, `frequency_1` and `tick` over `pulses` clock
/// pulses `period` frames apart, high for the first half.
fn frames(module: &mut dyn Module, period: usize, pulses: usize) -> Vec<[f32; 3]> {
    let mut played = Vec::new();
    for frame in 0..period * pulses {
        let high = frame % period < period / 2;
        module.input_block_mut(0)[0] = if high { 1.0 } else { 0.0 };
        module.process(1);
        let out = |port| module.output_block(port)[0];
        played.push([out(0), out(3), out(2)]);
    }
    played
}

fn rises(played: &[[f32; 3]], port: usize) -> Vec<usize> {
    (1..played.len())
        .filter(|&i| played[i][port] > 0.5 && played[i - 1][port] <= 0.5)
        .collect()
}

#[test]
fn rate_subdivides_the_clock_once_its_period_is_known() {
    let mut module = chance(serde_json::json!({ "seed": 1, "rate": "x4" }));
    let played = frames(module.as_mut(), 96, 4);
    // The first pulse steps once; each later one four times, 24 apart.
    let ticks = rises(&played, 2);
    assert_eq!(
        ticks,
        vec![96, 120, 144, 168, 192, 216, 240, 264, 288, 312, 336, 360]
    );
    // Subdivided, every gate lasts half a step rather than the pulse.
    assert!(played[96..108].iter().all(|out| out[2] == 1.0));
    assert!(played[108..120].iter().all(|out| out[2] == 0.0));
}

#[test]
fn jitter_delays_gates_within_half_a_step_and_moves_the_pitch_with_them() {
    let mut module = chance(serde_json::json!({ "seed": 6, "bias": 1.0, "jitter": 1.0 }));
    let played = frames(module.as_mut(), 100, 40);
    let gates = rises(&played, 0);
    assert!(gates.len() >= 38, "{gates:?}");
    let mut late = 0;
    for &rise in gates.iter().filter(|&&rise| rise >= 100) {
        let offset = rise % 100;
        assert!(offset <= 50, "{rise}");
        late += usize::from(offset > 0);
        // The pitch changes as the gate rises, never before.
        assert_eq!(
            played[rise - 1][1],
            played[rise.saturating_sub(offset + 1)][1]
        );
    }
    assert!(late > 30, "{late}");
}

#[test]
fn smooth_steps_glide_between_values() {
    let mut module = chance(serde_json::json!({ "seed": 8, "steps": 0.0, "bias": 1.0 }));
    let played = frames(module.as_mut(), 100, 6);
    // From the second pulse the pitch moves on most frames of a step.
    let moving = (201..300)
        .filter(|&i| played[i][1] != played[i - 1][1])
        .count();
    assert!(moving > 90, "{moving}");
}
