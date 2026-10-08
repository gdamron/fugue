//! The wall clock against a simulated device: conversions stay within the
//! bound `Transport::sample_at` documents.

use super::*;
use crate::alloc_counter::allocator_events;

const RATE: u32 = 48_000;

#[test]
fn without_a_wall_clock_nothing_converts() {
    let transport = Transport::new();
    assert_eq!(transport.sample_at(Instant::now()), None);
    assert_eq!(transport.samples_in(Duration::from_secs(1)), None);
}

#[test]
fn a_started_clock_converts_nothing_until_anchored() {
    let transport = Transport::new();
    let epoch = Instant::now();
    transport.start_clock(epoch, RATE);
    assert_eq!(transport.sample_at(epoch), None);
    assert_eq!(
        transport.samples_in(Duration::from_millis(250)),
        Some(12_000)
    );

    // Anchoring is allocation-free, and sets the relation.
    let heard = epoch + Duration::from_millis(500);
    let ((), allocs, frees) = allocator_events(|| transport.anchor(9_600, heard));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(transport.sample_at(heard), Some(9_600));
    assert_eq!(
        transport.sample_at(heard + Duration::from_millis(1)),
        Some(9_648)
    );
    // Sample 0 was heard at `epoch + 300 ms`: before it gives 0.
    assert_eq!(transport.sample_at(epoch), Some(0));
    let early = epoch.checked_sub(Duration::from_secs(1));
    assert!(early.is_none_or(|early| transport.sample_at(early) == Some(0)));
}

/// A device whose clock runs `DRIFT` fast against the system's, with a
/// fixed output latency, read `delay` after each callback starts: every
/// conversion, at leads from zero to ten seconds after an anchor, is within
/// `1 + RATE * (delay + lead * drift)` samples of the sample heard then
/// (the host's latency is exact here).
#[test]
fn wall_clock_conversion_stays_within_its_documented_bound() {
    const FRAMES: u64 = 512;
    const DRIFT: f64 = 100e-6;
    const MAX_DELAY: f64 = 100e-6;
    let device_rate = f64::from(RATE) * (1.0 + DRIFT);
    let latency = 0.011_6;
    // Seconds after `epoch` that device sample `s` is heard, and back.
    let heard = |s: u64| latency + s as f64 / device_rate;
    let heard_at = |t: f64| (t - latency) * device_rate;
    let epoch = Instant::now();
    let at = |t: f64| epoch + Duration::from_secs_f64(t);

    let transport = Transport::new();
    transport.start_clock(epoch, RATE);
    let mut seed = 0x2545_f491_u64;
    let mut worst = 0.0f64;
    for callback in 0..400 {
        let sample = callback * FRAMES;
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let delay = MAX_DELAY * (seed >> 11) as f64 / (1u64 << 53) as f64;
        let anchored = heard(sample) + delay;
        transport.anchor(sample, at(anchored));
        for lead in [0.0, 0.001, 0.005, 0.0107, 0.1, 1.0, 10.0] {
            let t = heard(sample) + lead;
            let got = transport.sample_at(at(t)).unwrap() as f64;
            let error = (got - heard_at(t)).abs();
            let bound = 1.0 + f64::from(RATE) * (delay + (t - anchored).abs() * DRIFT);
            assert!(
                error <= bound,
                "callback {callback}, lead {lead}: off by {error} > {bound}"
            );
            worst = worst.max(error / bound);
        }
    }
    // The bound is not vacuous: some conversion comes within half of it.
    assert!(worst > 0.5, "worst {worst}");
}
