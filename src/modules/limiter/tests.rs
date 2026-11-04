use super::*;

const SR: u32 = 48_000;

fn run(limiter: &mut Limiter, left: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    for chunk in left.chunks(MAX_BLOCK) {
        limiter.input_block_mut(0)[..chunk.len()].copy_from_slice(chunk);
        limiter.process(chunk.len());
        out.extend_from_slice(&limiter.output_block(0)[..chunk.len()]);
    }
    out
}

#[test]
fn the_lookahead_rounds_to_whole_frames_within_its_range() {
    assert_eq!(lookahead_frames(SR, 0.005), 240);
    assert_eq!(lookahead_frames(SR, 0.0), 24);
    assert_eq!(lookahead_frames(SR, f32::MAX), 960);
    assert_eq!(Limiter::new(SR, 0.005).latency(), 240);
}

#[test]
fn reduction_reports_the_block_s_deepest_cut() {
    let mut limiter = Limiter::new(SR, 0.001);
    limiter.set_control("ceiling", -6.0).unwrap();
    assert_eq!(limiter.get_control("reduction").unwrap(), 0.0);
    run(&mut limiter, &vec![1.0; 4800]);
    let reduction = limiter.get_control("reduction").unwrap();
    assert!((reduction - 6.0).abs() < 0.01, "reduction {reduction}");
    run(&mut limiter, &vec![0.0; 3 * 48_000]);
    assert_eq!(limiter.get_control("reduction").unwrap(), 0.0);
}

#[test]
fn writing_reduction_is_refused() {
    let mut limiter = Limiter::new(SR, 0.005);
    assert!(limiter.set_control("reduction", 3.0).is_err());
}

#[test]
fn a_lower_ceiling_glides_in_without_clipping_what_is_in_flight() {
    let mut limiter = Limiter::new(SR, 0.005);
    limiter.set_control("ceiling", 0.0).unwrap();
    run(&mut limiter, &vec![0.9; 2400]);
    limiter.set_control("ceiling", -12.0).unwrap();
    let out = run(&mut limiter, &vec![0.9; 4800]);
    let step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(step < 0.01, "largest step {step}");
    assert!((out[4799] - decibels_to_level(-12.0)).abs() < 1e-4);
}
