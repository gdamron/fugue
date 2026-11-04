use super::*;

pub(super) const RATE: u32 = 48_000;

/// Runs `input` through `delay` a block at a time and returns the output.
pub(super) fn run(delay: &mut Delay, input: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.chunks(MAX_BLOCK) {
        delay.input_block_mut(0)[..chunk.len()].copy_from_slice(chunk);
        delay.process(chunk.len());
        out.extend_from_slice(&delay.output_block(0)[..chunk.len()]);
    }
    out
}

fn impulse(len: usize) -> Vec<f32> {
    let mut signal = vec![0.0; len];
    signal[0] = 0.5;
    signal
}

#[test]
fn soft_clip_is_exact_within_the_knee_and_bounded_past_it() {
    for value in [0.0, 0.1, -0.3, 0.75, -0.75] {
        assert_eq!(soft_clip(value).to_bits(), value.to_bits());
    }
    let mut last = soft_clip(KNEE);
    for step in 1..=10_000 {
        let value = KNEE + step as f32 * 0.01;
        let clipped = soft_clip(value);
        assert!(
            clipped >= last && clipped < 1.0,
            "{value} clipped to {clipped}"
        );
        assert_eq!(soft_clip(-value), -clipped);
        last = clipped;
    }
    // No corner at the knee: the slope leaving it is one.
    let slope = (soft_clip(KNEE + 1e-4) - soft_clip(KNEE)) / 1e-4;
    assert!((slope - 1.0).abs() < 1e-2, "slope {slope}");
}

#[test]
fn a_whole_sample_time_delays_exactly() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("time", 100.0 / RATE as f32).unwrap();
    delay.heads.settle();
    let input: Vec<f32> = (0..1000).map(|n| ((n as f32) * 0.01).sin() * 0.7).collect();
    let out = run(&mut delay, &input);
    for n in 0..900 {
        assert_eq!(out[n + 100].to_bits(), input[n].to_bits(), "sample {n}");
    }
}

#[test]
fn a_dotted_eighth_at_120_bpm_is_18000_samples() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("division", 9.0).unwrap();
    assert_eq!(delay.time_samples(), 18_000.0);
    delay.heads.settle();
    let out = run(&mut delay, &impulse(20_000));
    assert_eq!(out[18_000], 0.5);
    assert!(out
        .iter()
        .enumerate()
        .all(|(n, &v)| n == 18_000 || v == 0.0));
}

#[test]
fn a_patched_tempo_overrides_the_control() {
    let mut delay = Delay::new(RATE, 2.0);
    delay.set_control("division", 3.0).unwrap(); // a quarter note
    delay.set_input_connected(2, true);
    delay.input_block_mut(2).fill(60.0);
    delay.process(1);
    assert_eq!(delay.time_samples(), f64::from(RATE));
    delay.set_input_connected(2, false);
    assert_eq!(delay.time_samples(), f64::from(RATE) / 2.0);
}

#[test]
fn time_is_at_least_one_sample_and_at_most_max_time() {
    let mut delay = Delay::new(RATE, 0.5);
    delay.set_control("time", 0.0).unwrap();
    assert_eq!(delay.time_samples(), 1.0);
    delay.set_control("time", 30.0).unwrap();
    assert_eq!(delay.time_samples(), f64::from(RATE) / 2.0);
}

#[test]
fn feedback_past_unity_stays_within_one() {
    let mut delay = Delay::new(RATE, 0.1);
    delay.set_control("time", 0.01).unwrap();
    delay.set_control("feedback", 1.1).unwrap();
    let input: Vec<f32> = (0..RATE as usize)
        .map(|n| if n < 480 { 0.9 } else { 0.0 })
        .collect();
    let out = run(&mut delay, &input);
    assert!(out.iter().all(|v| v.abs() < 1.0));
    assert!(
        out[out.len() - 480..].iter().any(|v| v.abs() > 0.5),
        "repeats died"
    );
}

#[test]
fn a_time_change_crossfades_without_a_step() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("time", 0.1).unwrap();
    delay.heads.settle();
    let tone: Vec<f32> = (0..RATE as usize)
        .map(|n| (n as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5)
        .collect();
    let first = run(&mut delay, &tone[..24_000]);
    delay.set_control("time", 0.2).unwrap();
    let second = run(&mut delay, &tone[24_000..]);
    let joined: Vec<f32> = first.into_iter().chain(second).collect();
    // A 440 Hz sine at 0.5 moves at most about 0.03 a sample.
    let largest = joined[5_000..]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max);
    assert!(largest < 0.06, "step of {largest}");
}

#[test]
fn config_max_time_is_bounded() {
    let factory = DelayFactory;
    for bad in [0.0, -1.0] {
        assert!(factory
            .build(RATE, &serde_json::json!({ "max_time": bad }))
            .is_err());
    }
    let built = factory
        .build(
            RATE,
            &serde_json::json!({ "max_time": 2.0, "division": "1/8.", "feedback": 0.4 }),
        )
        .unwrap();
    let GraphModule::Module(delay) = built.module else {
        panic!("a delay is a module");
    };
    assert_eq!(delay.get_control("division").unwrap(), 9.0);
    assert!((delay.get_control("feedback").unwrap() - 0.4).abs() < 1e-6);
}

pub(super) fn sine_437(len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| (n as f32 * 437.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5)
        .collect()
}

pub(super) fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// A finite input too large to shape with plain arithmetic still records
/// as a bounded sample, and the delay recovers once it has passed.
#[test]
fn huge_input_never_poisons_the_line() {
    for feedback in [0.0, 1.1] {
        let mut delay = Delay::new(RATE, 0.1);
        delay.set_control("time", 0.01).unwrap();
        delay.set_control("feedback", feedback).unwrap();
        let mut input = vec![0.0; 9_600];
        input[10] = f32::MAX;
        input[11] = -f32::MAX;
        input[12] = f32::MIN_POSITIVE;
        for (n, sample) in input.iter_mut().enumerate().skip(4_800) {
            *sample = (n as f32 * 0.05).sin() * 0.5;
        }
        let out = run(&mut delay, &input);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        assert!(
            out[9_000..].iter().any(|v| v.abs() > 0.1),
            "feedback {feedback}"
        );
    }
    assert_eq!(soft_clip(f32::MAX), 1.0);
    assert_eq!(soft_clip(f32::NEG_INFINITY), -1.0);
    assert_eq!(soft_clip(f32::NAN), 0.0);
}

/// Between samples the cubic read overshoots full-scale alternation; the
/// output stays within ±1 all the same.
#[test]
fn a_fractional_time_keeps_the_output_within_one() {
    let mut delay = Delay::new(RATE, 0.1);
    delay.set_control("time", 2.5 / RATE as f32).unwrap();
    delay.heads.settle();
    let input: Vec<f32> = (0..4_800)
        .map(|n| if (n / 2) % 2 == 0 { -4.0 } else { 4.0 })
        .collect();
    let out = run(&mut delay, &input);
    assert!(out.iter().all(|v| v.abs() <= 1.0));
}

/// Rising zero crossings of `signal`, interpolated between samples.
fn crossings(signal: &[f32]) -> Vec<f32> {
    (1..signal.len())
        .filter(|&n| signal[n - 1] < 0.0 && signal[n] >= 0.0)
        .map(|n| (n - 1) as f32 + signal[n - 1] / (signal[n - 1] - signal[n]))
        .collect()
}

#[test]
fn a_slewing_delay_starts_at_a_time_written_before_it_plays() {
    let sine: Vec<f32> = (0..RATE as usize)
        .map(|n| 0.5 * (std::f32::consts::TAU * 440.0 * n as f32 / RATE as f32).sin())
        .collect();
    let mut written = Delay::new(RATE, 1.0);
    written.set_control("glide", 1.0).unwrap();
    written.set_control("time", 0.15).unwrap();
    written.set_control("feedback", 0.5).unwrap();
    written.process(0);
    let mut built = Delay::new(RATE, 1.0);
    built.set_control("glide", 1.0).unwrap();
    built.set_control("time", 0.15).unwrap();
    built.set_control("feedback", 0.5).unwrap();
    built.heads.settle();
    built.feedback.settle();
    let heard = run(&mut written, &sine);
    assert_eq!(heard, run(&mut built, &sine));
    // The echo is at the sine's pitch from its first cycle: no slew in.
    let periods: Vec<f32> = crossings(&heard[7_200..9_600])
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    let expected = RATE as f32 / 440.0;
    assert!(periods.iter().all(|p| (p / expected - 1.0).abs() < 1e-3));
}

#[test]
fn a_patched_tempo_on_the_first_block_lands_at_once() {
    let mut delay = Delay::new(RATE, 1.0);
    delay.set_control("glide", 1.0).unwrap();
    delay.set_control("division", 3.0).unwrap();
    let bpm = INPUTS.iter().position(|&port| port == "bpm").unwrap();
    delay.set_input_connected(bpm, true);
    delay.input_block_mut(bpm).fill(90.0);
    let mut input = vec![0.0; 48_000];
    input[0] = 0.5;
    let mut out = Vec::new();
    for chunk in input.chunks(MAX_BLOCK) {
        delay.input_block_mut(0)[..chunk.len()].copy_from_slice(chunk);
        delay.input_block_mut(bpm)[..chunk.len()].fill(90.0);
        delay.process(chunk.len());
        out.extend_from_slice(&delay.output_block(0)[..chunk.len()]);
    }
    // A quarter at 90 bpm is 32,000 samples, exactly, from the start.
    assert_eq!(out[32_000], 0.5);
}
