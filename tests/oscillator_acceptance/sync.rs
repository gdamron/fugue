use fugue::dsp::{BandLimitedOscillator, Shape};

/// Runs a master at `m` and a saw slave at `s` (both phase increments), hard
/// synced via the master's `wrapped()`. With `synced` false the slave free-runs.
fn slave(m: f32, s: f32, len: usize, synced: bool) -> Vec<f32> {
    let (mut master, mut slave) = (BandLimitedOscillator::new(), BandLimitedOscillator::new());
    (0..len)
        .map(|_| {
            master.next(Shape::Saw, m, 0.5);
            match master.wrapped() {
                Some(since) if synced => slave.next_synced(Shape::Saw, s, 0.5, since),
                _ => slave.next(Shape::Saw, s, 0.5),
            }
        })
        .collect()
}

/// A master of exactly 64 samples per cycle makes the slave repeat every 64.
#[test]
fn slave_repeats_with_the_masters_period() {
    let max_diff = |y: &[f32]| {
        (640..2400)
            .map(|n| (y[n + 64] - y[n]).abs())
            .fold(0.0, f32::max)
    };
    // Control: unsynced, the slave (4.68 cycles per master cycle) does not repeat.
    assert!(max_diff(&slave(1.0 / 64.0, 0.0731, 2560, false)) > 0.1);
    let diff = max_diff(&slave(1.0 / 64.0, 0.0731, 2560, true));
    assert!(
        diff < 1e-3,
        "synced slave differs from itself one period later by {diff}"
    );
}
