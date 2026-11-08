//! Criterion: an invention whose output stays within the clamp's range
//! renders byte-identical to before the clamp existed. The hashes below were
//! computed on the unclamped render and pin it.

use super::support::*;

const FRAMES: usize = 24_000;

/// The pinned hashes were computed where they are enforced: libm's `sin`
/// differs between platforms in the last bit. Everywhere else the render is
/// still compared sample for sample with the unclamped live graph output.
pub const PINNED_PLATFORM: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

fn render_hash(json: &str) -> u64 {
    hash(&offline(json, FRAMES, 512))
}

fn assert_untouched(json: &str, pinned: u64, what: &str) {
    let out = offline(json, FRAMES, 512);
    assert!(
        out.iter().all(|x| (-1.0..=MAX_BELOW_ONE).contains(x)),
        "{what} stays within the clamp's range"
    );
    assert!(out.iter().any(|x| x.abs() > 0.1), "{what} is audible");
    if PINNED_PLATFORM {
        assert_eq!(hash(&out), pinned, "{what} renders as it always did");
    }
    assert_eq!(render_hash(json), hash(&out), "{what} repeats");
    let live = Live::start(json);
    let graph_output = live.capture(FRAMES - FRAMES % 64);
    live.running.stop();
    for (i, (got, raw)) in out.iter().zip(&graph_output).enumerate() {
        assert_eq!(got.to_bits(), raw.to_bits(), "{what} sample {i}");
    }
}

#[test]
fn a_quiet_sine_is_unchanged() {
    assert_untouched(&tone("sine", 440.0, -9.0), SINE, "quiet sine");
}

#[test]
fn a_soft_clipped_sine_is_unchanged() {
    assert_untouched(&soft_clipped_sine(), SOFT_CLIPPED, "soft-clipped sine");
}

#[test]
fn seeded_noise_is_unchanged() {
    assert_untouched(&noise(-6.0), NOISE, "seeded noise");
}

#[test]
fn a_waveform_at_a_modest_level_is_unchanged() {
    assert_untouched(&tone("sawtooth", 110.0, -6.0), SAWTOOTH, "sawtooth");
}

pub const SINE: u64 = 17960443412532431601;
pub const SOFT_CLIPPED: u64 = 8167537214704235809;
pub const NOISE: u64 = 12354198263809901537;
pub const SAWTOOTH: u64 = 11734814365905787393;
