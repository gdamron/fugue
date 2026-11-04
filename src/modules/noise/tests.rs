use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;

const RATE: u32 = 48_000;

fn render(noise: &mut Noise, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut white, mut pink) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut left = frames;
    while left > 0 {
        let block = left.min(MAX_BLOCK);
        noise.process(block);
        white.extend_from_slice(&noise.output_block(0)[..block]);
        pink.extend_from_slice(&noise.output_block(1)[..block]);
        left -= block;
    }
    (white, pink)
}

fn mean_and_rms(values: &[f32]) -> (f64, f64) {
    let n = values.len() as f64;
    let sum: f64 = values.iter().map(|&v| f64::from(v)).sum();
    let squares: f64 = values.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    (sum / n, (squares / n).sqrt())
}

#[test]
fn white_noise_at_full_level_is_centred_with_rms_of_one_over_root_three() {
    let mut noise = Noise::new(RATE, Some(11), 1.0);
    let (white, _) = render(&mut noise, 10 * RATE as usize);
    let (mean, rms) = mean_and_rms(&white);
    assert!(mean.abs() < 0.01, "mean {mean}");
    assert!((rms - 0.577).abs() < 0.02, "rms {rms}");
    assert!(white.iter().all(|v| v.abs() <= 1.0));
}

#[test]
fn the_same_seed_gives_the_same_bits_and_another_seed_does_not() {
    let (a, a_pink) = render(&mut Noise::new(RATE, Some(5), 1.0), 4096);
    let (b, b_pink) = render(&mut Noise::new(RATE, Some(5), 1.0), 4096);
    let (c, _) = render(&mut Noise::new(RATE, Some(6), 1.0), 4096);
    let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
    assert_eq!(bits(&a_pink), bits(&b_pink));
    assert_ne!(bits(&a), bits(&c));
}

#[test]
fn unseeded_sources_are_independent() {
    let (a, _) = render(&mut Noise::new(RATE, None, 1.0), 4096);
    let (b, _) = render(&mut Noise::new(RATE, None, 1.0), 4096);
    assert_ne!(a, b);
}

#[test]
fn a_level_write_glides_to_silence_without_allocating() {
    let mut noise = Noise::new(RATE, Some(1), 1.0);
    noise.set_control("level", 0.0).unwrap();
    let glide = gain_ramp_frames(RATE) as usize;
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        noise.process(glide.min(MAX_BLOCK));
    });
    assert_eq!((allocs, frees), (0, 0));
    let (white, pink) = render(&mut noise, MAX_BLOCK);
    assert!(white.iter().chain(&pink).all(|&v| v == 0.0));
    assert_eq!(noise.get_control("level").unwrap(), 0.0);
}

#[test]
fn a_configured_level_holds_from_the_first_frame() {
    let mut noise = Noise::new(RATE, Some(1), 0.0);
    let (white, pink) = render(&mut noise, 64);
    assert!(white.iter().chain(&pink).all(|&v| v == 0.0));
}

#[test]
fn the_factory_reads_seed_and_level() {
    let config = serde_json::json!({ "seed": 9, "level": 0.5 });
    let mut built = NoiseFactory.build(RATE, &config).unwrap();
    let module = built.module.module_mut();
    assert_eq!(module.outputs(), &["white", "pink"]);
    assert_eq!(module.get_control("level").unwrap(), 0.5);
    module.process(256);
    let mut reference = Noise::new(RATE, Some(9), 0.5);
    reference.process(256);
    assert_eq!(
        module.output_block(0)[..256],
        reference.output_block(0)[..256]
    );
}
